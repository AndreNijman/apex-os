#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-multilib-extract.sh — extract a REAL multilib set with the shipped
#  engine and assert that the 32-bit pass carried exactly what nothing else
#  provides: no shadow of an image file, and none of the arch-tagged files a
#  32-bit application cannot run without.
#
#  ── Why this exists next to test-apex-multilib.sh ───────────────────────────
#  That suite stops at guard_rpms on purpose, and says so: extracting a full set
#  through the normal path needs the container's installed versions to match the
#  repository, and when they do not the guard correctly refuses the transaction.
#  So the decisions are the discriminating step there. These defects are not in
#  the decisions. Every decision was right on katana on 2026-09-19 and the
#  machine still broke, twice, in opposite directions, because the second
#  (32-bit) pass of extract_rpms decided what to carry from a hand-written list
#  of --excludepath directories:
#
#    TOO NARROW (evidence §3). The list named /usr/bin, /usr/sbin, /usr/share
#    and /etc and nothing else, while i686 packages also ship helper
#    EXECUTABLES under /usr/libexec. A plain `apex install steam` shadowed 14
#    image binaries; gst-plugin-scanner is out of process, so GStreamer's
#    registry fell from 1344 usable features to 2.
#
#    TOO BLUNT (evidence §6.5). --excludepath /usr/share threw away every
#    *_icd.i686.json while keeping the .so files those manifests name, so
#    /usr/share/vulkan/icd.d held 13 x86_64 ICDs and ZERO i686 ones. Steam's
#    client is ubuntu12_32/steam, so it needs 32-bit Vulkan, and it died with
#    VK_ERROR_INCOMPATIBLE_DRIVER.
#
#  The engine now merges the 32-bit tree file by file under one rule: carry a
#  path only if neither the running image nor this transaction's own native pass
#  already provides it. This suite tests that rule from both sides.
#
#  extract_rpms is called DIRECTLY here, not through the install path, which is
#  what lets this run where the fuller suite would be refused: --nodeps
#  --replacefiles --replacepkgs into a throwaway root does not care whether the
#  container's versions match the repository's.
#
#  ── Every assertion is a discovery, not a list ──────────────────────────────
#  Nothing here names a directory or a filename the engine also names. The
#  packages and the container's own rpmdb are asked what they ship and what they
#  own, and the tree is judged against the answers. A future i686 package that
#  ships executables somewhere new, or a future Mesa that renames its manifests,
#  is covered without editing this file — which is the point, because the list
#  in the engine was the defect and a test that repeats a list proves nothing.
#
#  ── The two legs of the rule are separately controlled ──────────────────────
#  The engine drops a 32-bit file for one of two reasons, and in this container
#  they are exercised by different packages:
#
#    * gstreamer1.x86_64 and at-spi2-core.x86_64 are DOWNLOADED (fedora:43 does
#      not ship them), so their i686 helpers are dropped by the "already placed
#      by the native pass" clause.
#    * glib2.x86_64 is INSTALLED in fedora:43, so `download --resolve` skips it
#      and only glib2.i686 arrives. /usr/libexec/gio-launch-desktop is therefore
#      dropped by the rpmdb clause and by nothing else — the same clause that
#      does all the work on a real APEX machine, where every 64-bit sibling is
#      in the image rather than in the download.
#
#  Control 2 below re-runs the merge with the rpmdb clause REMOVED and requires
#  a shadow, so that leg cannot pass for free. Control 1 re-extracts with the
#  pre-fix exclude list and is read twice: it must still produce a /usr/libexec
#  shadow (so §3 is visible on today's packages) and it must contain none of the
#  arch-tagged Vulkan manifests (so §6.5 is visible too, and the assertion that
#  they are present is not satisfied by an exclusion that never applied).
#
#  ── And one assertion that is not about extraction at all ───────────────────
#  `apex resolve` lost its entire rpm leg because probe_rpm passed `--` to
#  `dnf5 repoquery`, which refuses it (evidence §11.1). test-apex-resolve.sh
#  could not see that: it fakes dnf5. This is the only suite in the tree with a
#  REAL dnf5 and a real repository, so probe_rpm is exercised against one here.
#
#  Run from anywhere: ./tests/test-apex-multilib-extract.sh
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
cd "$(dirname "$0")/.." || exit 2

ENGINE=files/system/libexec/apex-pkg
[ -f "$ENGINE" ] || { echo "cannot find $ENGINE"; exit 2; }

if ! command -v podman >/dev/null 2>&1; then
    echo "SKIP  podman is absent; this suite needs a real dnf and repository"
    exit 0
fi

IMAGE=${APEX_MULTILIB_IMAGE:-registry.fedoraproject.org/fedora:43}
PROBE=$(mktemp) || exit 2
trap 'rm -f "$PROBE"' EXIT

cat > "$PROBE" <<'PROBE_EOF'
set -uo pipefail
PKG=/repo/files/system/libexec/apex-pkg
WORK=/tmp/x; rm -rf "$WORK"; mkdir -p "$WORK/dl"

# gstreamer1 is the package the §3 defect was measured through and at-spi2-core
# is the one that left the greeter on a 32-bit accessibility bus; both ship an
# i686 build carrying /usr/libexec, and both have their x86_64 sibling
# downloaded here. mesa-vulkan-drivers is the §6.5 defect: its i686 build ships
# the arch-tagged Vulkan ICD manifests that a 32-bit client cannot start
# without. glib2.i686 comes in as a dependency and is the one whose x86_64
# sibling is already INSTALLED, which is what exercises the rpmdb clause.
dnf5 -y download --resolve --arch=x86_64 --arch=noarch --arch=i686 \
    --destdir "$WORK/dl" gstreamer1 at-spi2-core mesa-vulkan-drivers >/dev/null 2>&1 \
    || { echo "PROBE_SKIP no repository reachable"; exit 0; }

echo "PROBE_SET $(ls "$WORK/dl" | wc -l) rpms, $(ls "$WORK/dl" | grep -c i686) i686"

# ── What every source claims, before anything is judged ─────────────────────
# img.txt      every path a package INSTALLED in this container owns
# native.txt   every path a host-arch or noarch rpm in the set ships
# fonly.txt    paths ONLY a non-host-arch rpm in the set ships
rpm -qal 2>/dev/null | LC_ALL=C sort -u > "$WORK/img.txt"
if [ ! -s "$WORK/img.txt" ]; then
    echo "PROBE_SKIP the container rpmdb listed no files, so ownership cannot be judged"
    exit 0
fi
: > "$WORK/owned.txt"
for f in "$WORK/dl"/*.rpm; do
    a="$(rpm -qp --nosignature --qf '%{ARCH}' -- "$f" 2>/dev/null)"
    case "$a" in
        x86_64|noarch) tag=N ;;
        *)             tag=F ;;
    esac
    rpm -qlp --nosignature -- "$f" 2>/dev/null | sed "s/^/$tag /" >> "$WORK/owned.txt"
done
awk '$1=="N"{print $2}' "$WORK/owned.txt" | LC_ALL=C sort -u > "$WORK/native.txt"
awk '$1=="F"{print $2}' "$WORK/owned.txt" | LC_ALL=C sort -u > "$WORK/foreign.txt"
LC_ALL=C comm -23 "$WORK/foreign.txt" "$WORK/native.txt" > "$WORK/fonly.txt"
echo "PROBE_FOREIGN_ONLY $(wc -l < "$WORK/fonly.txt")"

# ELF class without `file(1)`, which the base image does not ship: byte 4 of an
# ELF header is EI_CLASS, 1 = 32-bit, 2 = 64-bit.
is_elf32() {
    local hdr
    hdr="$(od -An -tu1 -N5 -- "$1" 2>/dev/null | tr -s ' ')" || return 1
    [ "$(echo "$hdr" | cut -d' ' -f2-5)" = "127 69 76 70" ] || return 1
    [ "$(echo "$hdr" | cut -d' ' -f6)" = "1" ]
}

# Is this path claimed by something that is NOT the 32-bit set — the container's
# own rpmdb, or a native rpm in this very transaction? Either makes a 32-bit
# file at that path a shadow.
claimed_by() {
    local rel="$1"
    if LC_ALL=C grep -qxF -- "$rel" "$WORK/img.txt"; then echo image; return 0; fi
    if LC_ALL=C grep -qxF -- "$rel" "$WORK/native.txt"; then echo native; return 0; fi
    return 1
}

# Every 32-bit ELF in a tree, tagged with who else claims its path.
report_elf32() {
    local root="$1" tag="$2" f rel by
    while IFS= read -r -d '' f; do
        is_elf32 "$f" || continue
        rel="${f#"$root"}"
        if by="$(claimed_by "$rel")"; then
            echo "$tag SHADOW $by $rel"
        else
            echo "$tag CLEAR $rel"
        fi
    done < <(find "$root" -type f -print0 2>/dev/null)
}

# ── The engine, as shipped ──────────────────────────────────────────────────
# shellcheck disable=SC1090
source "$PKG" >/dev/null 2>&1
set +e
extract_rpms "$WORK/dl" "$WORK/root" >/dev/null 2>&1
echo "PROBE_EXTRACT_RC $?"
report_elf32 "$WORK/root" PROBE_ENGINE

# The non-ELF half of the same defect. Unit files, udev rules, tmpfiles and
# sysusers text cannot be found by looking at ELF headers, so ask the packages
# instead: a path only an i686 rpm ships, present in the tree, that an image
# package owns, was carried over the top of the operating system's own copy.
#
# FILES AND SYMLINKS ONLY, which is both what merge_multilib carries and what
# the katana measurement counted. Directories are excluded because a shared one
# is not a shadow: /usr/lib/.build-id/<xx> is a bucket every debuginfo-carrying
# package in Fedora owns a copy of, 55 of them are in this set alone, and an
# overlay merges directories rather than hiding them. Only the symlinks inside
# those buckets are real content, and each is named after a build id no other
# build has.
while IFS= read -r rel; do
    if [ -d "$WORK/root$rel" ] && [ ! -L "$WORK/root$rel" ]; then continue; fi
    if [ ! -e "$WORK/root$rel" ] && [ ! -L "$WORK/root$rel" ]; then continue; fi
    if LC_ALL=C grep -qxF -- "$rel" "$WORK/img.txt"; then
        echo "PROBE_FONLY_SHADOW $rel"
    fi
done < "$WORK/fonly.txt"

# The other direction — arch-tagged files that CANNOT shadow anything because
# nothing else on the system has that name, and that a 32-bit client is dead
# without. Derived from the rpms: everything only an i686 rpm ships under
# /usr/share/vulkan (icd.d manifests and implicit_layer.d manifests alike).
LC_ALL=C grep '^/usr/share/vulkan/' "$WORK/fonly.txt" > "$WORK/vulkan.txt"
echo "PROBE_VULKAN_EXPECT $(wc -l < "$WORK/vulkan.txt")"
while IFS= read -r rel; do
    [ -e "$WORK/root$rel" ] || echo "PROBE_VULKAN_MISSING $rel"
done < "$WORK/vulkan.txt"

# ── Control 1: the pre-fix exclude list, on the same set ────────────────────
# A replica of the command as it stood before this rule existed. Read twice —
# it must still produce a /usr/libexec shadow (§3 is visible on today's
# packages) and it must contain none of the Vulkan manifests above (§6.5 is
# visible too, so "they are all present" is not a free pass).
mkdir -p "$WORK/ctl"
rpm --root "$WORK/ctl" --initdb >/dev/null 2>&1
mapfile -t ctl_foreign < <(
    for f in "$WORK/dl"/*.rpm; do
        a="$(rpm -qp --nosignature --qf '%{ARCH}' -- "$f" 2>/dev/null)"
        [ "$a" = x86_64 ] || [ "$a" = noarch ] || echo "$f"
    done
)
if [ "${#ctl_foreign[@]}" -gt 0 ]; then
    rpm --root "$WORK/ctl" -Uvh --nodeps --noscripts --notriggers --noplugins \
        --nosignature --replacefiles --replacepkgs --excludepath /boot \
        --excludepath /usr/bin --excludepath /usr/sbin \
        --excludepath /usr/share --excludepath /etc \
        "${ctl_foreign[@]}" >/dev/null 2>&1
    report_elf32 "$WORK/ctl" PROBE_CONTROL
    n=0
    while IFS= read -r rel; do
        if [ -e "$WORK/ctl$rel" ]; then n=$((n+1)); fi
    done < "$WORK/vulkan.txt"
    echo "PROBE_CONTROL_VULKAN $n"
else
    echo "PROBE_CONTROL NONE no-foreign-rpms"
fi

# ── Control 2: the new rule with the rpmdb clause removed ───────────────────
# The engine drops a 32-bit file when the IMAGE owns the path or when the
# NATIVE pass already placed it. In this container the second clause alone
# would hide most of the defect, because the x86_64 siblings of gstreamer1 and
# at-spi2-core are in the download. This replica keeps only that second clause
# and must therefore let an image-owned 32-bit file through — glib2.i686's, on
# today's packages, because glib2.x86_64 is installed here and so was never
# downloaded. If it does NOT, the rpmdb clause is untested and every pass above
# is measuring the wrong leg of the rule.
mkdir -p "$WORK/ctl2" "$WORK/ctl2s"
rpm --root "$WORK/ctl2" --initdb >/dev/null 2>&1
rpm --root "$WORK/ctl2s" --initdb >/dev/null 2>&1
mapfile -t ctl2_native < <(
    for f in "$WORK/dl"/*.rpm; do
        a="$(rpm -qp --nosignature --qf '%{ARCH}' -- "$f" 2>/dev/null)"
        if [ "$a" = x86_64 ] || [ "$a" = noarch ]; then echo "$f"; fi
    done
)
if [ "${#ctl2_native[@]}" -gt 0 ] && [ "${#ctl_foreign[@]}" -gt 0 ]; then
    rpm --root "$WORK/ctl2" -Uvh --nodeps --noscripts --notriggers --noplugins \
        --nosignature --replacefiles --replacepkgs --excludepath /boot \
        "${ctl2_native[@]}" >/dev/null 2>&1
    rpm --root "$WORK/ctl2s" -Uvh --nodeps --noscripts --notriggers --noplugins \
        --nosignature --replacefiles --replacepkgs --excludepath /boot \
        "${ctl_foreign[@]}" >/dev/null 2>&1
    rm -rf "$WORK/ctl2s/usr/share/rpm" "$WORK/ctl2s/var/lib/rpm" \
           "$WORK/ctl2s/usr/lib/sysimage/rpm"
    while IFS= read -r rel; do
        if [ -e "$WORK/ctl2$rel" ] || [ -L "$WORK/ctl2$rel" ]; then continue; fi
        mkdir -p "$WORK/ctl2$(dirname "$rel")"
        cp -a "$WORK/ctl2s$rel" "$WORK/ctl2$rel" 2>/dev/null
    done < <( cd "$WORK/ctl2s" && find . -mindepth 1 \( -type f -o -type l \) -printf '/%P\n' )
    report_elf32 "$WORK/ctl2" PROBE_CONTROL2
else
    echo "PROBE_CONTROL2 NONE not-enough-rpms"
fi

# ── Defect §11.1: probe_rpm against a REAL dnf5 ─────────────────────────────
# Every other assertion about the resolver runs against a fake dnf5 that
# ignores its arguments, which is exactly how `dnf5 repoquery -- <name>` — an
# invocation dnf5 REFUSES — survived long enough to reach a user's machine and
# make `apex resolve` disagree with `apex install`.
echo "PROBE_RESOLVE $(probe_rpm gstreamer1 | head -1)"
PROBE_EOF

out=$(podman run --rm --privileged \
        -v "$PWD":/repo:ro,Z -v "$PROBE":/probe.sh:ro,Z \
        "$IMAGE" bash /probe.sh 2>&1)

# NOT `printf … | grep -q`: a match makes grep exit before printf finishes, so
# the writer dies of SIGPIPE and pipefail hands the `if` a 141. That inversion
# is position-dependent, which is the worst kind — it would turn a SKIP into a
# silent pass some of the time.
if [[ "$out" == *PROBE_SKIP* ]]; then
    echo "SKIP  $(printf '%s\n' "$out" | grep PROBE_SKIP | sed 's/PROBE_SKIP //')"
    exit 0
fi

pass=0; fail=0
ok()  { printf 'PASS  %s\n' "$1"; pass=$((pass+1)); }
bad() { printf 'FAIL  %s\n' "$1"; fail=$((fail+1)); }
count() { printf '%s\n' "$out" | grep -c "$1"; }
show()  { printf '%s\n' "$out" | grep "$1" | sed "s|$1||" | sed 's/^/        /' | head -12; }

printf '%s\n' "$out" | grep PROBE_SET | sed 's/PROBE_SET/      set:/'

rc=$(printf '%s\n' "$out" | grep PROBE_EXTRACT_RC | awk '{print $2}')
elf_clear=$(count '^PROBE_ENGINE CLEAR ')
elf_shadow=$(count '^PROBE_ENGINE SHADOW ')
printf '      extract rc %s; 32-bit ELF in tree: %s claimed by nothing else, %s shadowing\n' \
       "${rc:-?}" "$elf_clear" "$elf_shadow"

if [ "${rc:-1}" = 0 ]; then
    ok "extract_rpms completed on a real multilib set"
else
    bad "extract_rpms failed (rc ${rc:-unknown}), so nothing below was measured"
fi

# Can-only-pass guard. An empty tree satisfies "nothing is shadowed" without
# testing anything, and this set contains i686 libraries by construction.
if [ "$((elf_clear + elf_shadow))" -gt 0 ]; then
    ok "the set really did carry 32-bit ELF into the tree ($((elf_clear + elf_shadow)))"
else
    bad "no 32-bit ELF reached the tree at all, so the placement rule was never exercised"
fi

if [ "$elf_shadow" -eq 0 ]; then
    ok "no 32-bit ELF sits on a path the image or a 64-bit package in the set also ships"
else
    bad "$elf_shadow 32-bit ELF file(s) shadow a 64-bit provider:"
    show '^PROBE_ENGINE SHADOW '
fi

fonly=$(printf '%s\n' "$out" | grep '^PROBE_FOREIGN_ONLY ' | awk '{print $2}')
fshadow=$(count '^PROBE_FONLY_SHADOW ')
printf '      paths only an i686 rpm ships: %s; of those, present AND image-owned: %s\n' \
       "${fonly:-?}" "$fshadow"
if [ "${fonly:-0}" -gt 0 ]; then
    ok "the set contains i686-only paths, so the non-ELF half had something to find"
else
    bad "no i686-only paths in the set at all, so the non-ELF half was never exercised"
fi
if [ "$fshadow" -eq 0 ]; then
    ok "no i686-only path was carried over a path the image owns"
else
    bad "$fshadow i686-only path(s) landed on top of an image-owned file:"
    show '^PROBE_FONLY_SHADOW '
fi

# §6.5, the other direction: the arch-tagged manifests must SURVIVE.
vexpect=$(printf '%s\n' "$out" | grep '^PROBE_VULKAN_EXPECT ' | awk '{print $2}')
vmissing=$(count '^PROBE_VULKAN_MISSING ')
vctl=$(printf '%s\n' "$out" | grep '^PROBE_CONTROL_VULKAN ' | awk '{print $2}')
printf '      arch-tagged Vulkan manifests only an i686 rpm ships: %s; missing from the tree: %s (control tree has %s)\n' \
       "${vexpect:-?}" "$vmissing" "${vctl:-?}"
if [ "${vexpect:-0}" -gt 0 ]; then
    ok "the set ships i686-only Vulkan manifests, so this assertion had something to find"
else
    bad "the set ships no i686-only Vulkan manifest, so §6.5 was never exercised"
fi
if [ "$vmissing" -eq 0 ]; then
    ok "every i686-only Vulkan manifest the set ships reached the tree"
else
    bad "$vmissing i686-only Vulkan manifest(s) were thrown away — a 32-bit client sees no ICD:"
    show '^PROBE_VULKAN_MISSING '
fi

# ── The controls prove the instrument, every run, on today's packages ───────
ctl_shadow=$(count '^PROBE_CONTROL SHADOW ')
if [ "$ctl_shadow" -gt 0 ]; then
    ok "control 1: the pre-fix exclude list still produces $ctl_shadow shadow(s), so this suite can see one"
else
    bad "control 1 produced NO shadow, so the shadow assertions above are blind and mean nothing"
fi
if [ "${vctl:-1}" = 0 ]; then
    ok "control 1: the pre-fix exclude list threw away all ${vexpect:-?} Vulkan manifests, so keeping them is a real change"
else
    bad "control 1 kept ${vctl:-?} Vulkan manifest(s), so the pre-fix rule did not lose them and the assertion above passes for free"
fi

ctl2_shadow=$(count '^PROBE_CONTROL2 SHADOW image ')
if [ "$ctl2_shadow" -gt 0 ]; then
    ok "control 2: dropping the rpmdb clause lets $ctl2_shadow image-owned 32-bit file(s) through, so that clause is load-bearing here"
else
    bad "control 2 produced no image-owned shadow, so the rpmdb clause is untested in this container and only the native-pass clause was measured"
fi

# ── §11.1, on the way past: apex resolve's rpm leg, against a real dnf5 ─────
resolved=$(printf '%s\n' "$out" | grep '^PROBE_RESOLVE ' | sed 's/^PROBE_RESOLVE //')
printf '      probe_rpm gstreamer1 (real dnf5): %s\n' "${resolved:-<nothing>}"
if [[ "$resolved" == gstreamer1\|* ]]; then
    ok "probe_rpm finds a repository candidate through a real dnf5, so 'apex resolve' still has an rpm leg"
else
    bad "probe_rpm returned no candidate against a real dnf5 — 'apex resolve' would omit every RPM and predict a source 'apex install' does not use"
fi

echo
printf 'apex-multilib-extract: %d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
