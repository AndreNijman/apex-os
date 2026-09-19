#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-pkg-etc-label.sh — install a REAL package set with the shipped
#  engine and assert that nothing it puts in /etc carries a label the SELinux
#  policy disagrees with.
#
#  ── The defect ──────────────────────────────────────────────────────────────
#  ROADMAP/evidence/katana-pwd-lock-selinux-20260920.md. install_etc copies its
#  payload with `cp -a`, which preserves the SOURCE label, and the source is an
#  rpm extraction tree under /var/lib. So every file `apex install` put in /etc
#  arrived as rpm_var_lib_t. Seven of katana's eleven tracked paths were wrong,
#  and one of them was /etc/.pwd.lock, which policy wants as passwd_file_t.
#  systemd takes that lock before it will allocate a dynamic UID, so all six
#  DynamicUser=yes units on the machine — capsule@.service, rpm-ostreed.service,
#  fwupd-refresh.service among them — died at step USER with 217/USER. Nothing
#  logged an AVC. `systemctl --failed` showed nothing, because the units are
#  on-demand. It cost four rounds of probing to find, and it recurred on the
#  next rebuild with a different wrong type, which is why the fix is in the
#  engine and not a restorecon on one machine.
#
#  ── Why this suite does not check /etc/.pwd.lock ────────────────────────────
#  Because that file is where the defect happened to BITE, not where it lives.
#  The property asserted here is over EVERYTHING the pass writes: after an
#  install, no path install_etc placed in /etc may have a type different from
#  what `matchpathcon` — the policy — says it should have. The population is
#  discovered twice over: the packages decide which paths exist, and the
#  filesystem's own ctime decides which of them this pass touched. Nothing in
#  this file names a path that the engine also names, for the same reason the
#  multilib suite names no directory: a hardcoded map of filenames in the engine
#  would have been the same mistake as the --excludepath list it had to abandon,
#  and a test that repeats such a map proves only that two lists match.
#
#  .pwd.lock appears once below, in a control, as the reproduction of the
#  measured katana finding — not as the assertion.
#
#  ── What a container can and cannot prove ───────────────────────────────────
#  Both, and the suite says which it did. `stat -c %C` and `restorecon` need
#  two different things:
#
#    ENFORCED mode. The host kernel has SELinux, /sys/fs/selinux is mounted into
#    the container, and /etc inside it is a bind mount of a host-backed volume —
#    podman's own rootfs is mounted with a fixed `context=`, where setxattr
#    security.selinux returns EOPNOTSUPP and NOTHING can be relabelled. With
#    those two, the real property is measured: labels are set by the engine and
#    read back per file.
#
#    COVERAGE mode. No SELinux on the host (a GitHub ubuntu-24.04 runner), or
#    APEX_ETC_LABEL_FORCE_COVERAGE=1. Labels cannot be set or read at all, so
#    what is asserted instead is that every path the pass wrote was HANDED to
#    the policy labeller, and that the policy has a non-trivial opinion about
#    that set. `restorecon` and `selinuxenabled` are shimmed to record and to
#    answer yes; matchpathcon still works from the policy files with SELinux
#    switched off, so the second half is a real measurement.
#
#  The suite refuses to fall back silently: if the HOST has SELinux and the
#  enforced leg still did not run, that is a failure, not a skip.
#
#  ── What neither mode can confirm, and only a real machine can ──────────────
#    * that a DynamicUser=yes unit starts afterwards. There is no PID 1 here.
#    * that the extraction tree comes out rpm_var_lib_t on its own. That is a
#      measured fact from katana; this suite chcon's the source tree to it so
#      the pre-fix control reproduces the exact finding.
#    * that restorecon resolves against the APEX image's loaded policy. Here it
#      resolves against Fedora 43's policy rpm.
#
#  Run from anywhere: ./tests/test-apex-pkg-etc-label.sh
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
cd "$(dirname "$0")/.." || exit 2

ENGINE=files/system/libexec/apex-pkg
[ -f "$ENGINE" ] || { echo "cannot find $ENGINE"; exit 2; }

if ! command -v podman >/dev/null 2>&1; then
    echo "SKIP  podman is absent; this suite needs a real dnf, a real repository and a real SELinux policy"
    exit 0
fi

IMAGE=${APEX_ETC_LABEL_IMAGE:-registry.fedoraproject.org/fedora:43}

# Host-backed, because the container's own filesystem cannot hold a per-file
# SELinux label (see the header). Files inside it end up owned by mapped
# subuids under rootless podman, so the cleanup has to go through
# `podman unshare` — a plain rm leaves a directory nobody can delete.
VOL=$(mktemp -d) || exit 2
PROBE=$(mktemp) || exit 2
cleanup() {
    rm -f "$PROBE"
    podman unshare rm -rf "$VOL" >/dev/null 2>&1 || rm -rf "$VOL" 2>/dev/null
}
trap cleanup EXIT

# The host's SELinux state decides which leg is even possible, and it is read
# HERE so the suite can tell the difference between "this machine cannot
# measure labels" and "this machine can and something went wrong".
# /sys/fs/selinux/enforce, not the DIRECTORY: ubuntu kernels are built with
# CONFIG_SECURITY_SELINUX=y and run AppArmor, and whether the empty mount point
# exists there is not something this suite should bet a red CI run on. The file
# exists only when selinuxfs is actually mounted.
HOST_SELINUX=no
[ -e /sys/fs/selinux/enforce ] && HOST_SELINUX=yes

cat > "$PROBE" <<'PROBE_EOF'
set -uo pipefail
PKG=/repo/files/system/libexec/apex-pkg
FORCE_COVERAGE="${APEX_ETC_LABEL_FORCE_COVERAGE:-0}"

dnf -y install --setopt=install_weak_deps=False \
        policycoreutils libselinux-utils selinux-policy-targeted >/dev/null 2>&1 \
    || { echo "PROBE_SKIP the SELinux userspace would not install in this container"; exit 0; }
FC=/etc/selinux/targeted/contexts/files/file_contexts
[ -s "$FC" ] || { echo "PROBE_SKIP this container has no file_contexts, so the policy cannot be asked anything"; exit 0; }

WORK=/tmp/x; rm -rf "$WORK"; mkdir -p "$WORK/dl"

# Chosen for what the POLICY says about the /etc these packages ship, not for
# what the files are called: between them they produce paths policy wants as
# bin_t (profile.d scripts), passwd_file_t and shadow_t (the passwd/group/shadow
# family a user-creating transaction rewrites), system_cron_spool_t (cron.d),
# dhcp_etc_t and plain etc_t. chrony and cronie create users, which is what
# makes rpm write /etc/.pwd.lock into the extraction tree — the katana file,
# arriving here for the same reason it arrived there. fontconfig is in the set
# because it ships /etc SYMLINKS, the case install_etc's own `cp -a, NEVER
# install` comment is about.
dnf5 -y download --arch=x86_64 --arch=noarch --destdir "$WORK/dl" \
        chrony cronie logrotate vim-enhanced which sudo openssh-server fontconfig \
        >/dev/null 2>&1 \
    || { echo "PROBE_SKIP no repository reachable"; exit 0; }
echo "PROBE_SET $(find "$WORK/dl" -name '*.rpm' | wc -l)"

# shellcheck disable=SC1090
source "$PKG" >/dev/null 2>&1
set +e

# extract_rpms and split_out_of_image are called DIRECTLY, exactly as the
# multilib extract suite calls extract_rpms: the install path would need this
# container's versions to match the repository, and the guard correctly refuses
# the transaction when they do not. The decisions are not what is on trial here.
extract_rpms "$WORK/dl" "$WORK/root" >/dev/null 2>&1
echo "PROBE_EXTRACT_RC $?"
split_out_of_image "$WORK/root" "$WORK/etcsrc" >/dev/null 2>&1

mkdir -p /vol/src
cp -a "$WORK/etcsrc/." /vol/src/ 2>/dev/null
echo "PROBE_SRC $(find /vol/src -mindepth 1 | wc -l)"
# Reproduce the label a real extraction tree carries. On katana the tree is
# under /var/lib/apex/pkg/work and comes out rpm_var_lib_t on its own; here the
# tree would be container_file_t, which is wrong in a different way and would
# make the pre-fix control pass for a reason the defect never had.
chcon -R -t rpm_var_lib_t /vol/src >/dev/null 2>&1
src_probe="$(find /vol/src -type f 2>/dev/null | head -1)"
echo "PROBE_SRC_LABEL $(stat -c %C "$src_probe" 2>/dev/null | cut -d: -f3)"

# A pristine copy of this container's /etc, so every pass can start from the
# same place an install starts from on a real machine.
mkdir -p /vol/etc0 && cp -a /etc/. /vol/etc0/ 2>/dev/null

MOUNTED=0
BOUND=0

pass_end() {
    [ "$MOUNTED" = 1 ] || return 0
    umount /var/lib/apex 2>/dev/null
    umount /etc 2>/dev/null
    MOUNTED=0
}

# Start a pass from a known state. In enforced mode /etc and /var/lib/apex are
# re-bound from fresh copies on the host-backed volume and relabelled to what
# the policy says, because a real machine's /etc is policy-correct BEFORE
# `apex install` runs and without that every pre-existing file would count as
# this pass's mislabel. In coverage mode nothing can be labelled anyway, so the
# container's own /etc is used and only the engine's state is reset.
pass_begin() {
    pass_end
    if [ "$BOUND" = 1 ]; then
        rm -rf /vol/e /vol/a
        mkdir -p /vol/e /vol/a
        cp -a /vol/etc0/. /vol/e/ 2>/dev/null
        mount --bind /vol/e /etc  || return 1
        mkdir -p /var/lib/apex
        mount --bind /vol/a /var/lib/apex || { umount /etc 2>/dev/null; return 1; }
        MOUNTED=1
        # -F, not a bare restorecon: see the comment on the noforce control.
        restorecon -F -R /etc /var/lib/apex >/dev/null 2>&1
    else
        rm -rf /var/lib/apex/pkg
    fi
    return 0
}

# Path and ctime for everything under /etc, as one fork. Compared before and
# after a run, this is the filesystem's own record of what the run wrote — no
# clock, no marker file, and no re-derivation of install_etc's branch logic.
snapshot() { find /etc -mindepth 1 -printf '%C@\t%p\n' 2>/dev/null | LC_ALL=C sort > "$1"; }

mark() { snapshot /tmp/before.txt; }

# Every path under /etc this pass actually wrote. `cp -a` preserves mtime but
# never ctime, so a changed ctime is the honest record of "we wrote this", and
# a path absent from the before-snapshot is one this pass created.
#
# Directories are the exception, and it is not a small one: a directory's ctime
# moves when a child is created inside it, so a pre-existing /etc/profile.d
# would otherwise read as something this pass wrote and then be demanded of a
# relabel pass that correctly left it alone. A directory counts only when it is
# NEW.
touched() {
    snapshot /tmp/after.txt
    LC_ALL=C awk -F'\t' '
        NR==FNR { c[$2]=$1; next }
        { if (!($2 in c)) print "N\t" $2; else if (c[$2] != $1) print "C\t" $2 }
    ' /tmp/before.txt /tmp/after.txt | while IFS="$(printf '\t')" read -r how p; do
        if [ -d "$p" ] && [ ! -L "$p" ]; then
            [ "$how" = N ] && printf '%s\n' "$p"
        else
            printf '%s\n' "$p"
        fi
    done
    return 0
}

# The policy's verdict on each path it is given. Types only: the defect, and
# everything it broke, is a type mismatch, and comparing seuser or MCS would
# make this report churn that has no security meaning. The full contexts are
# printed so a failure can be read without re-running anything.
verify() {
    local tag="$1" t cur want
    while IFS= read -r t; do
        cur="$(stat -c %C "$t" 2>/dev/null)"
        want="$(matchpathcon -n "$t" 2>/dev/null)"
        [ -n "$cur" ] && [ -n "$want" ] || continue
        [ "$(printf '%s' "$cur" | cut -d: -f3)" = "$(printf '%s' "$want" | cut -d: -f3)" ] \
            && continue
        printf '%s %s cur=%s want=%s\n' "$tag" "$t" "$cur" "$want"
    done
}

# How many of these paths the policy wants as something OTHER than plain etc_t.
# If that is zero the whole property is trivial — "label it etc_t" would pass —
# and every assertion below is worth nothing.
nondefault() {
    local t want
    while IFS= read -r t; do
        want="$(matchpathcon -n "$t" 2>/dev/null | cut -d: -f3)"
        [ -n "$want" ] && [ "$want" != etc_t ] && printf '%s\n' "$t"
    done
    return 0
}

# ── which leg is possible here ──────────────────────────────────────────────
# A capability probe, not a reading of selinuxenabled: what matters is whether
# a file can be mislabelled and then put right, which is the operation the
# engine performs and the operation the assertions read back.
MODE=coverage
if [ "$FORCE_COVERAGE" != 1 ]; then
    BOUND=1
    if pass_begin; then
        : > /etc/.apex-label-capability
        chcon -t rpm_var_lib_t /etc/.apex-label-capability >/dev/null 2>&1
        cap_before="$(stat -c %C /etc/.apex-label-capability 2>/dev/null | cut -d: -f3)"
        restorecon -F /etc/.apex-label-capability >/dev/null 2>&1
        cap_after="$(stat -c %C /etc/.apex-label-capability 2>/dev/null | cut -d: -f3)"
        cap_want="$(matchpathcon -n /etc/.apex-label-capability 2>/dev/null | cut -d: -f3)"
        rm -f /etc/.apex-label-capability
        if [ "$cap_before" = rpm_var_lib_t ] && [ -n "$cap_after" ] && [ "$cap_after" = "$cap_want" ]; then
            MODE=enforced
        fi
        echo "PROBE_CAPABILITY before=${cap_before:-none} after=${cap_after:-none} want=${cap_want:-none}"
    else
        echo "PROBE_CAPABILITY bind-failed"
    fi
    pass_end
fi
[ "$MODE" = enforced ] || BOUND=0
echo "PROBE_MODE $MODE"

run_engine_twice() {
    # Twice, because the second run is where katana broke. On the first, /etc
    # already holds the file and nothing of ours is saved, so install_etc
    # writes <path>.apexnew and leaves the original alone; on the second the
    # saved copy exists and matches, so the original IS overwritten — and that
    # is the run that put rpm_var_lib_t on /etc/.pwd.lock.
    local tag="$1" run
    for run in 1 2; do
        mark
        install_etc /vol/src >/dev/null 2>&1
        touched | LC_ALL=C sort -u > "/tmp/touched.${run}"
        echo "PROBE_${tag}_TOUCHED ${run} $(wc -l < "/tmp/touched.${run}")"
        echo "PROBE_${tag}_NONDEFAULT ${run} $(nondefault < "/tmp/touched.${run}" | wc -l)"
        if [ "$MODE" = enforced ]; then
            verify "PROBE_${tag}_MISLABEL${run}" < "/tmp/touched.${run}"
        fi
    done
}

# ── leg 1: the engine as shipped ────────────────────────────────────────────
if [ "$MODE" = enforced ]; then
    pass_begin
    run_engine_twice ENGINE
    # And then the whole of /etc, not only what the pass touched. The baseline
    # was relabelled at pass_begin and nothing else writes here, so a mismatch
    # anywhere in the tree is this engine's.
    find /etc -mindepth 1 2>/dev/null > /tmp/all.txt
    echo "PROBE_SWEEP $(wc -l < /tmp/all.txt) $(verify SWEEP < /tmp/all.txt | wc -l)"

    # Negative control, the literal one: break a single file that the pass just
    # got right and require the checker to name it and nothing else. Without
    # this, "no mismatches" could equally mean the checker reads no labels.
    victim="$(head -1 /tmp/touched.2)"
    chcon -t rpm_var_lib_t "$victim" >/dev/null 2>&1
    echo "PROBE_VICTIM $victim"
    printf '%s\n' "$victim" | verify PROBE_VICTIM_SEEN
    verify PROBE_VICTIM_SWEEP < /tmp/touched.2
    pass_end
fi

# ── leg 2, control: the engine without the relabel pass ─────────────────────
# The pre-fix replica. Everything else is identical, so what it produces is
# exactly what shipped before this fix.
if [ "$MODE" = enforced ]; then
    eval "$(declare -f relabel_etc | sed '1s/^relabel_etc/relabel_etc_real/')"
    relabel_etc() { :; }
    pass_begin
    run_engine_twice PREFIX
    # The katana finding itself, reproduced rather than asserted: the file that
    # took down every DynamicUser=yes unit on the machine, after the same two
    # runs, with the labelling pass removed.
    echo "PROBE_PREFIX_PWDLOCK cur=$(stat -c %C /etc/.pwd.lock 2>/dev/null | cut -d: -f3) want=$(matchpathcon -n /etc/.pwd.lock 2>/dev/null | cut -d: -f3)"
    pass_end

    eval "$(declare -f relabel_etc_real | sed '1s/^relabel_etc_real/relabel_etc/')"

    # ── leg 3, why the engine passes -F ─────────────────────────────────────
    # A bare `restorecon` silently declines to touch any file whose current
    # type is in the policy's customizable_types list — "not reset as
    # customized by admin", one line per file, and then exit 0. That exemption
    # exists for labels an admin chose deliberately; a file apex-pkg has just
    # written is not one of those, and inheriting the exemption would make the
    # relabel pass run, report success and change nothing.
    #
    # The type is read OUT OF THE POLICY rather than named here, because which
    # types are customizable is the policy's decision and changes with it.
    pass_begin
    ctype="$(grep -v '^#' /etc/selinux/targeted/contexts/customizable_types 2>/dev/null | awk 'NF{print $1; exit}')"
    : > /etc/.apex-customizable-probe
    chcon -t "${ctype:-container_file_t}" /etc/.apex-customizable-probe >/dev/null 2>&1
    restorecon /etc/.apex-customizable-probe >/dev/null 2>&1
    echo "PROBE_NOFORCE type=${ctype:-none} after_plain=$(stat -c %C /etc/.apex-customizable-probe 2>/dev/null | cut -d: -f3)"
    restorecon -F /etc/.apex-customizable-probe >/dev/null 2>&1
    echo "PROBE_FORCE after_forced=$(stat -c %C /etc/.apex-customizable-probe 2>/dev/null | cut -d: -f3) want=$(matchpathcon -n /etc/.apex-customizable-probe 2>/dev/null | cut -d: -f3)"
    rm -f /etc/.apex-customizable-probe
    pass_end
fi

# ── leg 4: coverage — everything written is handed to the policy ────────────
# The leg that runs anywhere, and the only one a runner without SELinux can
# have. `restorecon` is replaced by a recorder and `selinuxenabled` by a yes,
# so the engine takes exactly the path it takes on a labelled machine and this
# pass can read back the set of paths it submitted.
mkdir -p /tmp/shim
cat > /tmp/shim/restorecon <<'SHIM'
#!/usr/bin/env bash
log=/tmp/submitted.txt
while [ "$#" -gt 0 ]; do
    case "$1" in
        -f) shift; [ -n "${1:-}" ] && cat -- "$1" >> "$log" ;;
        -R) shift; [ -n "${1:-}" ] && printf 'R %s\n' "$1" >> "$log" ;;
        -*) : ;;
        *)  printf '%s\n' "$1" >> "$log" ;;
    esac
    shift
done
exit 0
SHIM
cat > /tmp/shim/selinuxenabled <<'SHIM'
#!/usr/bin/env bash
exit 0
SHIM
chmod +x /tmp/shim/restorecon /tmp/shim/selinuxenabled
PATH=/tmp/shim:$PATH

coverage_pass() {
    local tag="$1" run
    : > /tmp/submitted.txt
    pass_begin
    for run in 1 2; do
        mark
        install_etc /vol/src >/dev/null 2>&1
        touched | LC_ALL=C sort -u > "/tmp/cov.${run}"
        echo "PROBE_${tag}_TOUCHED ${run} $(wc -l < "/tmp/cov.${run}")"
        echo "PROBE_${tag}_NONDEFAULT ${run} $(nondefault < "/tmp/cov.${run}" | wc -l)"
        while IFS= read -r t; do
            grep -qxF -- "$t" /tmp/submitted.txt || echo "PROBE_${tag}_UNSUBMITTED${run} $t"
        done < "/tmp/cov.${run}"
    done
    echo "PROBE_${tag}_SUBMITTED $(LC_ALL=C sort -u /tmp/submitted.txt | wc -l)"
    pass_end
}

coverage_pass COV

# ── leg 5, control: the gate must see ONE forgotten path ────────────────────
# The realistic shape of a regression here is not "the pass was deleted" but
# "a branch was added that forgets to record what it wrote". So: drop the last
# path from the list the engine hands over, and require leg 4's comparison to
# name a path it no longer sees.
eval "$(declare -f relabel_etc | sed '1s/^relabel_etc/relabel_etc_real2/')"
relabel_etc() {
    local list="$1"
    [ -s "$list" ] || return 0
    tail -1 -- "$list" >> /tmp/dropped.txt
    head -n -1 -- "$list" > "${list}.mut"
    relabel_etc_real2 "${list}.mut"
}
: > /tmp/dropped.txt
coverage_pass COVCTL
echo "PROBE_COVCTL_DROPPED $(head -1 /tmp/dropped.txt)"
eval "$(declare -f relabel_etc_real2 | sed '1s/^relabel_etc_real2/relabel_etc/')"
PROBE_EOF

# /sys/fs/selinux is mounted READ-WRITE or libselinux reports SELinux as
# disabled and restorecon becomes a silent no-op (measured: ro -> selinuxenabled
# returns 1). It adds no reach that --privileged did not already grant — a
# privileged container can mount selinuxfs itself — and nothing in the probe
# above writes to it.
# shellcheck disable=SC2054  # the commas are inside podman -v option strings
podman_args=(--rm --privileged
    -v "$PWD":/repo:ro,Z
    -v "$VOL":/vol:z
    -v "$PROBE":/probe.sh:ro,Z
    -e "APEX_ETC_LABEL_FORCE_COVERAGE=${APEX_ETC_LABEL_FORCE_COVERAGE:-0}")
[ "$HOST_SELINUX" = yes ] && podman_args+=(-v /sys/fs/selinux:/sys/fs/selinux)

out=$(podman run "${podman_args[@]}" "$IMAGE" bash /probe.sh 2>&1)

# NOT `printf … | grep -q`: a match makes grep exit before printf finishes, the
# writer dies of SIGPIPE, and pipefail hands the `if` a 141.
if [[ "$out" == *PROBE_SKIP* ]]; then
    echo "SKIP  $(printf '%s\n' "$out" | grep PROBE_SKIP | sed 's/PROBE_SKIP //')"
    exit 0
fi

pass=0; fail=0
ok()  { printf 'PASS  %s\n' "$1"; pass=$((pass+1)); }
bad() { printf 'FAIL  %s\n' "$1"; fail=$((fail+1)); }
count() { printf '%s\n' "$out" | grep -c "$1"; }
field() { printf '%s\n' "$out" | grep "^$1 " | head -1 | cut -d' ' -f"$2"; }
show()  { printf '%s\n' "$out" | grep "$1" | sed "s|$1||" | sed 's/^/        /' | head -10; }

mode=$(field PROBE_MODE 2)
printf '      mode: %s   package set: %s rpms   /etc paths the set ships: %s   extraction tree label: %s\n' \
       "${mode:-?}" "$(field PROBE_SET 2)" "$(field PROBE_SRC 2)" "$(field PROBE_SRC_LABEL 2)"
printf '%s\n' "$out" | grep '^PROBE_CAPABILITY ' | sed 's/^PROBE_CAPABILITY /      relabel capability: /'

rc=$(field PROBE_EXTRACT_RC 2)
if [ "${rc:-1}" = 0 ]; then
    ok "extract_rpms completed on a real package set"
else
    bad "extract_rpms failed (rc ${rc:-unknown}), so nothing below was measured"
fi

if [ "${mode:-}" = enforced ] || [ "${mode:-}" = coverage ]; then
    ok "the probe decided a measurement mode (${mode})"
else
    bad "the probe printed no mode at all, so it never got as far as deciding what it could measure"
fi

# A machine that CAN measure labels and did not is a failure, not a skip. Every
# silent fallback in this repository's history has been a green tick over
# nothing measured.
if [ "$HOST_SELINUX" = yes ] && [ "${APEX_ETC_LABEL_FORCE_COVERAGE:-0}" != 1 ]; then
    if [ "${mode:-}" = enforced ]; then
        ok "this host has SELinux and the label leg ran on it"
    else
        bad "this host has SELinux, nothing forced coverage mode, and the label leg still did not run — the strongest assertions in this suite were skipped silently"
    fi
fi

# ── the property, where it can be measured ──────────────────────────────────
if [ "${mode:-}" = enforced ]; then
    for run in 1 2; do
        touched=$(field "PROBE_ENGINE_TOUCHED ${run}" 3)
        nondef=$(field "PROBE_ENGINE_NONDEFAULT ${run}" 3)
        mis=$(count "^PROBE_ENGINE_MISLABEL${run} ")
        printf '      install %s: %s path(s) written in /etc, %s of them a type policy wants as something other than etc_t, %s mislabelled\n' \
               "$run" "${touched:-?}" "${nondef:-?}" "$mis"
        if [ "${touched:-0}" -gt 0 ]; then
            ok "install ${run} wrote something in /etc, so the property had something to judge"
        else
            bad "install ${run} wrote nothing in /etc at all — every assertion about it below passes for free"
        fi
        if [ "${nondef:-0}" -gt 0 ]; then
            ok "install ${run}: policy wants a non-default type for ${nondef} of those paths, so 'label it etc_t' would not pass"
        else
            bad "install ${run}: policy wants plain etc_t for every path written, so this set cannot tell a policy lookup from a constant"
        fi
        if [ "$mis" -eq 0 ]; then
            ok "install ${run}: every path written in /etc carries the label matchpathcon says it should"
        else
            bad "install ${run}: ${mis} path(s) in /etc do not match the policy:"
            show "^PROBE_ENGINE_MISLABEL${run} "
        fi
    done

    sweep_n=$(field PROBE_SWEEP 2); sweep_bad=$(field PROBE_SWEEP 3)
    printf '      whole-tree sweep: %s path(s) under /etc, %s mislabelled\n' "${sweep_n:-?}" "${sweep_bad:-?}"
    if [ "${sweep_bad:-1}" = 0 ]; then
        ok "after two installs nothing anywhere under /etc disagrees with the policy (${sweep_n} paths)"
    else
        bad "${sweep_bad} path(s) under /etc disagree with the policy after two installs"
    fi

    victim=$(field PROBE_VICTIM 2)
    seen=$(count '^PROBE_VICTIM_SEEN ')
    swept=$(count '^PROBE_VICTIM_SWEEP ')
    printf '      negative control: mislabelled %s by hand; checker saw it %s time(s), sweep named %s path(s)\n' \
           "${victim:-?}" "$seen" "$swept"
    if [ "$seen" -ge 1 ] && [ "$swept" = 1 ]; then
        ok "breaking one file by hand turns the checker red for that file and only that file, so it reads labels per path"
    else
        bad "a deliberately mislabelled file was seen ${seen} time(s) and the sweep named ${swept} path(s) — expected 1 and 1, so the checker is not reading what it claims to"
    fi

    # ── control: the engine without the pass ────────────────────────────────
    pre1=$(count '^PROBE_PREFIX_MISLABEL1 '); pre2=$(count '^PROBE_PREFIX_MISLABEL2 ')
    pwd_line=$(printf '%s\n' "$out" | grep '^PROBE_PREFIX_PWDLOCK ' | head -1)
    printf '      pre-fix control: %s mislabelled after install 1, %s after install 2\n' "$pre1" "$pre2"
    printf '      pre-fix control, the katana file: %s\n' "${pwd_line#PROBE_PREFIX_PWDLOCK }"
    if [ "$pre2" -gt 0 ]; then
        ok "removing the relabel pass puts ${pre2} mislabelled file(s) back in /etc, so the pass is what makes the property hold"
    else
        bad "removing the relabel pass changed nothing, so the assertions above are not measuring it and something else is labelling these files"
    fi
    if [[ "$pwd_line" == *"cur=rpm_var_lib_t want=passwd_file_t"* ]]; then
        ok "pre-fix control reproduces the katana finding exactly: /etc/.pwd.lock rpm_var_lib_t where policy wants passwd_file_t"
    else
        bad "pre-fix control did NOT reproduce the katana finding — the package set no longer produces that file, or it arrives some other way, and the control has stopped standing for the defect it was written for"
    fi

    # ── why the engine passes -F ────────────────────────────────────────────
    nf_type=$(printf '%s\n' "$out" | grep '^PROBE_NOFORCE ' | head -1 | sed 's/^PROBE_NOFORCE //')
    nf_plain=$(printf '%s\n' "$out" | grep -o 'after_plain=[a-z_]*' | head -1 | cut -d= -f2)
    nf_forced=$(printf '%s\n' "$out" | grep -o 'after_forced=[a-z_]*' | head -1 | cut -d= -f2)
    nf_want=$(printf '%s\n' "$out" | grep '^PROBE_FORCE ' | head -1 | grep -o 'want=[a-z_]*' | cut -d= -f2)
    printf '      -F control: %s; a bare restorecon left it %s, restorecon -F made it %s (policy: %s)\n' \
           "${nf_type:-?}" "${nf_plain:-?}" "${nf_forced:-?}" "${nf_want:-?}"
    if [ -n "$nf_plain" ] && [ -n "$nf_want" ] && [ "$nf_plain" != "$nf_want" ] && [ "$nf_forced" = "$nf_want" ]; then
        ok "a bare restorecon declines to reset a customizable type and -F does reset it, which is why the engine passes -F"
    else
        bad "the customizable-type case did not behave as the engine's -F comment claims (plain left ${nf_plain:-?}, forced left ${nf_forced:-?}, policy wants ${nf_want:-?}) — either restorecon changed or the engine's reason for that flag is wrong"
    fi
fi

# ── coverage: runs everywhere, and is all a runner without SELinux has ──────
for run in 1 2; do
    ctouched=$(field "PROBE_COV_TOUCHED ${run}" 3)
    cnondef=$(field "PROBE_COV_NONDEFAULT ${run}" 3)
    unsub=$(count "^PROBE_COV_UNSUBMITTED${run} ")
    printf '      coverage, install %s: %s path(s) written in /etc, %s with a non-default policy type, %s never handed to the labeller\n' \
           "$run" "${ctouched:-?}" "${cnondef:-?}" "$unsub"
    if [ "${ctouched:-0}" -gt 0 ]; then
        ok "coverage, install ${run}: the pass wrote ${ctouched} path(s), so the comparison had a population"
    else
        bad "coverage, install ${run}: the pass wrote nothing, so 'everything was submitted' is vacuous"
    fi
    if [ "${cnondef:-0}" -gt 0 ]; then
        ok "coverage, install ${run}: the policy wants a non-default type for ${cnondef} of them, so the lookup is not a constant"
    else
        bad "coverage, install ${run}: the policy wants plain etc_t everywhere, so this set cannot distinguish a policy lookup from a constant"
    fi
    if [ "$unsub" -eq 0 ]; then
        ok "coverage, install ${run}: every path the pass wrote in /etc was handed to the policy labeller"
    else
        bad "coverage, install ${run}: ${unsub} path(s) were written and never submitted for labelling:"
        show "^PROBE_COV_UNSUBMITTED${run} "
    fi
done

ctl_unsub=$(( $(count '^PROBE_COVCTL_UNSUBMITTED1 ') + $(count '^PROBE_COVCTL_UNSUBMITTED2 ') ))
printf '      coverage control: dropped %s from the list the engine submits -> %s unsubmitted path(s) found\n' \
       "$(field PROBE_COVCTL_DROPPED 2)" "$ctl_unsub"
if [ "$ctl_unsub" -ge 1 ]; then
    ok "dropping ONE path from what the engine submits turns the coverage gate red, so it can see a single forgotten file"
else
    bad "dropping a path from what the engine submits changed nothing — the coverage assertions above are blind"
fi

if [ "${mode:-}" != enforced ]; then
    echo "NOTE  labels could not be read or set here, so the assertions that compare a"
    echo "      file's actual label against matchpathcon did not run. What ran instead"
    echo "      is the coverage leg above: every path the pass wrote was handed to the"
    echo "      policy labeller, and the policy's answers for that set are non-trivial."
    echo "      Only a machine with SELinux confirms the label is applied, and only a"
    echo "      machine with PID 1 confirms DynamicUser=yes works afterwards."
fi

echo
printf 'apex-pkg-etc-label: %d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
