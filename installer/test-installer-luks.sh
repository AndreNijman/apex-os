#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-installer-luks.sh — the encryption decision, its refusals, and the
#  keyboard conversion that decides whether the owner can type their own
#  passphrase. Everything here runs WITHOUT touching a block device.
#
#  The companion suite test-installer-luks-live.sh does the other half: a real
#  `bootc install` onto a real LUKS2 volume on a loopback file. It needs root,
#  a 15 GB image and ~20 GB of disk, so it cannot run on a CI runner and is
#  listed in tests/suites-not-in-ci.txt. THIS file is the half CI runs, and it
#  is deliberately the half that guards the refusals — because every refusal
#  here is a case where the alternative is a disk the owner cannot open.
#
#  ── what is under test ──
#
#  1. `encrypt=` is MANDATORY. Not "defaults to no", not "defaults to yes".
#     Both silent defaults are wrong in opposite directions and the engine
#     refuses to pick one. Asserted in both arms: missing is refused, and a
#     bogus value is refused.
#  2. Every refusal fires BEFORE anything is written. Each case's expected text
#     contains "Nothing has been erased" or stops at the block-device check,
#     and none of them names a real device.
#  3. The passphrase rules, which exist because the unlock prompt is a kernel
#     console prompt: printable ASCII only (no accents, no input method, no
#     compose key at that prompt) and at least 8 characters.
#  4. An APEX-OS image with no /usr/libexec/apex-luks-enroll cannot produce a
#     recovery key, so it may not produce an encrypted disk either. That
#     refusal is asserted, and so is its inverse — with a helper present the
#     same answers reach the dry-run stop.
#  5. XKB layout -> console keymap. `gb` is `uk`, `ch` is `sg`, `latam` is
#     `la-latin1`, `jp` is `jp106`; writing the XKB name into vconsole.conf
#     gives loadkeys a name it does not know, it fails, and the console stays
#     on `us` — which on an encrypted machine is the owner locked out of their
#     own disk. The conversion function is SOURCED OUT OF THE SHIPPED ENGINE,
#     not copied here, so this tests what installs.
#  6. MUTATION. Every engine-level assertion is re-run against a COPY of the
#     engine with the guard under test removed, and must then fail. A guard
#     that cannot be made to fail is not being tested. The original engine is
#     never modified: the mutant is a separate file.
#
#  PASS = every case behaves as named AND every mutant flips the result.
#  Run from the repo's installer/ directory. Needs passwordless root (sudo -n)
#  and podman, for the same reason test-installer.sh does: the engine refuses
#  to run unprivileged and checks for an image before parsing arguments.
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
cd "$(dirname "$0")" || exit 1

ENGINE=./apex-install
WORK=$(mktemp -d /tmp/apex-luks-suite.XXXXXX)
ANS="$WORK/answers"
SCRATCH_IMAGE="localhost/apex-luks-probe:test"
ENGINE_IMAGE=""
scratch_made=0
cleanup() {
    rm -rf "$WORK"
    [ "$scratch_made" = 1 ] && sudo -n podman rmi -f "$SCRATCH_IMAGE" >/dev/null 2>&1
    return 0
}
trap cleanup EXIT

pass=0; fail=0
ok()  { printf 'PASS  %-46s %s\n' "$1" "${2:-}"; pass=$((pass+1)); }
bad() { printf 'FAIL  %-46s %s\n' "$1" "${2:-}"; fail=$((fail+1)); }

# Same trick test-installer.sh uses: the engine's preflight refuses to continue
# without an APEX-OS image in ROOT podman storage, and that check runs before
# argument parsing. An empty tar imported by podman is a valid image with no
# layers — no network, no build, removed on exit.
ensure_engine_image() {
    command -v podman >/dev/null 2>&1 || return 1
    sudo -n true 2>/dev/null || return 1
    if sudo -n podman image exists localhost/apex-os:daily 2>/dev/null; then
        ENGINE_IMAGE="localhost/apex-os:daily"; return 0
    fi
    local t; t=$(mktemp "$WORK/empty.XXXXXX.tar") || return 1
    tar -cf "$t" -T /dev/null 2>/dev/null \
        && sudo -n podman import -q "$t" "$SCRATCH_IMAGE" >/dev/null 2>&1
    local rc=$?
    rm -f "$t"
    [ "$rc" = 0 ] || return 1
    scratch_made=1
    ENGINE_IMAGE="$SCRATCH_IMAGE"
    return 0
}

ENGINE_RUNNABLE=1
if ! ensure_engine_image; then
    ENGINE_RUNNABLE=0
    echo "SKIP: the engine half cannot run here — it needs an APEX-OS image in ROOT"
    echo "      podman storage and passwordless sudo. The keymap half still runs."
fi

# A stand-in for /usr/libexec/apex-luks-enroll that the engine can find without
# an image carrying one. It is only ever reached by the dry-run cases, which
# stop before any enrolment happens; the live suite exercises a real one.
HELPER="$WORK/enroll-stub"
printf '#!/bin/sh\nexit 0\n' > "$HELPER"; chmod 755 "$HELPER"

# ── run one answers file against an engine (the real one, or a mutant) ───────
run_engine() {  # $1=engine path  $2=answers body  [$3..]=extra env assignments
    local eng="$1" body="$2"; shift 2
    printf '%s\n' "$body" > "$ANS"
    sudo -n APEX_IMAGE="$ENGINE_IMAGE" "$@" "$eng" --headless "$ANS" 2>&1 </dev/null
}

# $1 name, $2 expected substring, $3 answers body, $4.. extra env
check() {
    local name=$1 want=$2 body=$3; shift 3
    if [ "$ENGINE_RUNNABLE" != 1 ]; then printf 'SKIP  %-46s no engine image\n' "$name"; return; fi
    local out; out=$(run_engine "$ENGINE" "$body" "$@")
    # The ERR trap firing is always a bug in the installer, never a pass — the
    # same rule test-installer.sh states.
    if [[ "$out" == *"Unexpected error on line"* ]]; then
        bad "$name" "ERR TRAP FIRED"; return
    fi
    if [[ "$out" == *"$want"* ]]; then ok "$name"
    else bad "$name" "wanted '$want'; got: $(printf '%s' "$out" | grep -m1 'APEX-INSTALL-' || echo '<no sentinel>')"
    fi
}

# Mutation: copy the engine, delete the guard under test, and require the SAME
# answers to stop behaving that way. `cp` then `cmp` on the original afterwards,
# so a mutation can never leak into the shipped file.
mutate_check() {  # $1 name  $2 sed program  $3 no-longer-expected substring  $4 answers  $5.. env
    local name=$1 prog=$2 gone=$3 body=$4; shift 4
    if [ "$ENGINE_RUNNABLE" != 1 ]; then printf 'SKIP  %-46s no engine image\n' "mutant: $name"; return; fi
    local mut="$WORK/mutant-engine" out
    cp "$ENGINE" "$mut" || { bad "mutant: $name" "could not copy the engine"; return; }
    chmod 755 "$mut"
    sed -i "$prog" "$mut"
    if cmp -s "$ENGINE" "$mut"; then
        bad "mutant: $name" "the mutation changed nothing — the sed program matched no line"
        return
    fi
    out=$(run_engine "$mut" "$body" "$@")
    if [[ "$out" == *"$gone"* ]]; then
        bad "mutant: $name" "the guard still fired with its code removed — the case proves nothing"
    else
        ok "mutant: $name" "guard removed -> refusal gone"
    fi
    rm -f "$mut"
}

# A disk that cannot exist: the whole-disk cases that are SUPPOSED to get past
# the encryption guards then stop at the block-device check, so nothing real is
# ever named, let alone opened.
GHOST=/dev/zzz-does-not-exist
BASE=$'mode=disk\ndisk='"$GHOST"$'\nusername=bob\npassword=pw\nhostname=apex'

echo "── the encryption decision is explicit, or refused ────────────────────"
check "encrypt= missing is refused"        "encrypt missing"      "$BASE"
check "encrypt=maybe is refused"           "bad encrypt 'maybe'"  "$BASE"$'\nencrypt=maybe'
check "encrypt=no reaches the disk check"  "is not a block device" "$BASE"$'\nencrypt=no'
# The mutation is a ONE-LINE inversion, not a block delete: a `,+Nd` range over
# a multi-line die string eats the next statement too, the mutant then fails to
# parse, and a mutant that cannot run looks exactly like a mutation that worked.
mutate_check "encrypt= missing" \
    's/\[ -n "\${ENCRYPT:-}" \]/[ -z "${ENCRYPT:-}" ]/' "encrypt missing" "$BASE"

echo
echo "── encryption needs a whole disk, and says so before erasing ──────────"
if [ -b /dev/sda ] && [ -b /dev/sda2 ] && [ -b /dev/sda1 ]; then
    check "partition mode + encrypt=yes refused" "only available when APEX-OS gets a whole disk" \
        $'mode=partition\ndisk=/dev/sda\ntarget=/dev/sda2\nesp=/dev/sda1\nusername=bob\npassword=pw\nhostname=apex\nencrypt=yes\nlukspass=correcthorse'
else
    echo "SKIP  partition+encrypt case (needs /dev/sda1 and /dev/sda2 present)"
fi

echo
echo "── the passphrase must be typeable at a console prompt ────────────────"
check "lukspass missing"      "lukspass missing"             "$BASE"$'\nencrypt=yes'
check "lukspass too short"    "too short"                    "$BASE"$'\nencrypt=yes\nlukspass=abc'
check "lukspass non-ASCII"    "cannot be typed at the boot prompt" "$BASE"$'\nencrypt=yes\nlukspass=pässwörd1'
check "lukspass emoji"        "cannot be typed at the boot prompt" "$BASE"$'\nencrypt=yes\nlukspass=abcdefgh\xf0\x9f\x94\x92'
check "good lukspass gets past the passphrase rules" "is not a block device" \
    "$BASE"$'\nencrypt=yes\nlukspass=correct horse 9'
mutate_check "non-ASCII passphrase" \
    's/if LC_ALL=C grep -qv/if false \&\& LC_ALL=C grep -qv/' \
    "cannot be typed at the boot prompt" "$BASE"$'\nencrypt=yes\nlukspass=pässwörd1'
mutate_check "short passphrase" \
    's/if \[ "\${#LUKSPASS}" -lt 8 \]; then/if false; then/' \
    "too short" "$BASE"$'\nencrypt=yes\nlukspass=abc'

echo
echo "── a non-btrfs root filesystem is not an exercised combination ────────"
check "encrypt=yes + ext4 refused" "only supported with the btrfs root filesystem" \
    "$BASE"$'\nencrypt=yes\nlukspass=correcthorse\nrootfs=ext4'

echo
echo "── the dry run, the keymap it resolved, and the helper check ──────────"
# APEX_DRY_RUN runs every guard against the REAL device named and stops
# immediately before the first destructive command. A loopback file is made
# here only so the device checks have something to look at; that nothing was
# written to it is itself asserted below.
LOOPIMG=""; LOOPDEV=""
if [ "$ENGINE_RUNNABLE" = 1 ] && command -v losetup >/dev/null 2>&1; then
    LOOPIMG="$WORK/dryrun.img"
    truncate -s 20G "$LOOPIMG" 2>/dev/null && LOOPDEV=$(sudo -n losetup -fP --show "$LOOPIMG" 2>/dev/null || true)
fi
if [ -n "$LOOPDEV" ]; then
    # A REAL regular file, never a process substitution: the engine checks
    # `[ -f "$ANS" ]`, /dev/fd/N is not a regular file, and the fd would not
    # survive sudo anyway — the case would "fail" for a reason that has nothing
    # to do with what it is testing.
    #
    # keymap=bg on purpose. `bg` is one of the 36 XKB layout names (of 99) that
    # is NOT a loadable console keymap, so it is a layout where the conversion
    # has to do real work: the answer must come back as bg_bds-utf8.
    printf '%s\n' "mode=disk" "disk=$LOOPDEV" "username=bob" "password=pw" \
        "hostname=apex" "encrypt=yes" "lukspass=correct horse 9" "keymap=bg" > "$ANS"

    out=$(sudo -n APEX_IMAGE="$ENGINE_IMAGE" APEX_DRY_RUN=1 APEX_LUKS_ENROLL_LOCAL="$HELPER" \
             "$ENGINE" --headless "$ANS" 2>&1 </dev/null)
    if [[ "$out" == *"APEX-INSTALL-DRYRUN-OK"* ]]; then ok "encrypt=yes reaches the dry-run stop"
    else bad "encrypt=yes reaches the dry-run stop" "$(printf '%s' "$out" | tail -3 | tr '\n' ' ')"; fi
    if [[ "$out" == *"ENCRYPT=yes"* ]]; then ok "the dry run names the encryption decision"
    else bad "the dry run names the encryption decision" "no ENCRYPT= in the summary"; fi
    if [[ "$out" == *"KEYMAP=bg_bds-utf8"* ]]; then ok "XKB bg resolves to console keymap bg_bds-utf8 (end to end)"
    else bad "XKB bg resolves to console keymap bg_bds-utf8 (end to end)" \
             "$(printf '%s' "$out" | grep -o 'KEYMAP=[a-z0-9_.-]*' | tail -1)"; fi
    # The dry run stops before the first destructive command, and this is the
    # assertion that says so rather than trusting the name of the flag.
    if [ -z "$(sudo -n blkid -p "$LOOPDEV" 2>/dev/null || true)" ]; then
        ok "the dry run wrote nothing to the disk it validated"
    else
        bad "the dry run wrote nothing to the disk it validated" "blkid sees something on $LOOPDEV"
    fi

    # ── the helper check, both ways, on the same answers file ─────────────
    # The run above passed WITH a helper. These two differ from it only in the
    # helper, so that pass cannot be an accident of something else letting it
    # through.
    out=$(sudo -n APEX_IMAGE="$ENGINE_IMAGE" APEX_DRY_RUN=1 APEX_LUKS_ENROLL_LOCAL=/nonexistent/x \
             "$ENGINE" --headless "$ANS" 2>&1 </dev/null)
    if [[ "$out" == *"is not executable"* ]]; then ok "an unusable helper stops the same run"
    else bad "an unusable helper stops the same run" "$(printf '%s' "$out" | tail -2 | tr '\n' ' ')"; fi
    # With no override the engine asks the IMAGE, which today carries no such
    # helper — whichever image was picked above. The refusal must name the path
    # it looked for, because "encryption failed" with no path is unactionable.
    out=$(sudo -n APEX_IMAGE="$ENGINE_IMAGE" APEX_DRY_RUN=1 \
             "$ENGINE" --headless "$ANS" 2>&1 </dev/null)
    if [[ "$out" == *"/usr/libexec/apex-luks-enroll"* ]]; then
        ok "an image without the helper is refused, by name"
    else
        bad "an image without the helper is refused, by name" "$(printf '%s' "$out" | tail -2 | tr '\n' ' ')"
    fi
    sudo -n losetup -d "$LOOPDEV" 2>/dev/null || true
else
    echo "SKIP  dry-run cases (need losetup and an engine image)"
fi

echo
echo
echo "── a loopback target must not be able to reach this machine's NVRAM ───"
# WHY THIS IS IN THE ENCRYPTION SUITE. The encrypted path is the one that runs
# `bootc install to-filesystem` inside a `--privileged --pid=host` container,
# and the live half of this suite points that at a LOOPBACK FILE on a
# developer's own machine. On 2026-09-20 exactly that shape of run — a
# privileged loopback install with the host's efivarfs visible — deleted a
# laptop's real `APEX-OS` boot entry and recreated it against the loop device's
# ESP. The laptop would not boot. BOOT-BREAKAGE-2026-09-20.md.
#
# The engine now masks efivars when, and only when, the target is loop-backed.
# These assertions run the REAL FUNCTIONS OUT OF THE SHIPPED ENGINE against a
# fabricated sysfs tree, so they measure behaviour and not the presence of a
# string in a file.
NVFNS="$WORK/nvram-fns.sh"
FAKESYS="$WORK/sysblock"
mkdir -p "$FAKESYS/loop9/loop"
printf '/var/lab-scratch/pretend.img\n' > "$FAKESYS/loop9/loop/backing_file"

# nvram_probe ENGINE DEVICE [APEX_SYSFS_BLOCK] — prints the NVRAM_ARGS the
# named engine would use for that device, or `EXTRACT-FAILED`.
nvram_probe() {
    local eng="$1" dev="$2" seam="${3:-}"
    sed -n '/^disk_is_loopback()/,/^}/p;/^set_nvram_args_for()/,/^}/p' "$eng" > "$NVFNS"
    grep -q 'set_nvram_args_for' "$NVFNS" || { echo "EXTRACT-FAILED"; return; }
    APEX_SYSFS_BLOCK="$seam" bash -c '
        log()  { :; }
        note() { :; }
        . "$1"
        set_nvram_args_for "$2"
        printf "%s %s\n" "${NVRAM_BOOTC_ARGS[*]-}" "${NVRAM_ARGS[*]-}"
    ' _ "$NVFNS" "$dev" 2>/dev/null
}

got=$(nvram_probe "$ENGINE" /dev/loop9 "$FAKESYS")
# THE PREVENTION, and it is the bootc flag and not the mount. MEASURED
# 2026-09-20 22:12: a loopback install ran with the tmpfs mask applied and
# logged, and STILL executed `efibootmgr --create --disk /dev/loop1` against
# this machine. bootc needs --pid=host and re-enters the host mount namespace
# for the bootloader step, so the container's own mounts are not where bootupd
# looks. `bootc install --generic-image` skips the firmware step outright.
case "$got" in
    *"--generic-image"*)
        ok "a loop-backed target skips the firmware step" "$got" ;;
    EXTRACT-FAILED*)
        bad "a loop-backed target skips the firmware step" "could not extract the functions from the engine" ;;
    *)  bad "a loop-backed target skips the firmware step" "got '${got:-<empty>}'" ;;
esac
# Defence in depth, kept and asserted, but never again mistaken for the guard.
case "$got" in
    *"--tmpfs /sys/firmware/efi/efivars"*)
        ok "and still masks efivars in the container" "second layer, not the first" ;;
    *)  bad "and still masks efivars in the container" "got '${got:-<empty>}'" ;;
esac

# The inverse, and it is the one that must not regress: a REAL disk still gets
# the firmware, because a real install has to create a boot entry or the
# machine it just installed will not start.
got=$(nvram_probe "$ENGINE" /dev/nvme0n1 "$FAKESYS")
if [ -z "$(printf '%s' "$got" | tr -d '[:space:]')" ]; then
    ok "a real block device still reaches the firmware" "no extra arguments added"
else
    bad "a real block device still reaches the firmware" "engine would have added '$got'"
fi

# The seam only ever ADDS loop-ness: with the override unset, the fabricated
# tree is invisible and /dev/loop9 (which does not exist here) is not loop.
# A seam that could HIDE a loop device would be able to re-create the incident.
got=$(nvram_probe "$ENGINE" /dev/loop9 "")
if [ -z "$(printf '%s' "$got" | tr -d '[:space:]')" ]; then
    ok "the test seam cannot hide a real loop device" "unset override -> real sysfs only"
else
    bad "the test seam cannot hide a real loop device" "got '$got' with the override unset"
fi

# STRUCTURAL: a fifth install call site added later without the mask would pass
# every behavioural assertion above and still brick a laptop. So the engine is
# read as a whole: every privileged container is found, its full (backslash-
# continued) command line is reassembled, and each one is classified.
#
# An install container — one whose command is `bootc install` — must be
# immediately preceded by set_nvram_args_for AND must pass NVRAM_ARGS.
#
# There is exactly one privileged container that is NOT an install: the call to
# the enrolment helper. It is named here rather than exempted by position,
# because it deliberately keeps the host's efivarfs — /usr/libexec/apex-luks-
# enroll decides whether to add a TPM keyslot by reading the SecureBoot EFI
# variable, and masking that would silently turn every TPM enrolment off. It
# runs no bootloader tool. Any OTHER privileged container appearing in this
# engine is an unreviewed NVRAM risk and fails this assertion by name.
nvscan=$(awk '
  { l[NR] = $0 }
  END {
    for (i = 1; i <= NR; i++) {
      if (l[i] !~ /run --rm --privileged/) continue
      site++
      cmd = l[i]; j = i
      while (l[j] ~ /\\$/ && j < NR) { j++; cmd = cmd " " l[j] }
      if (cmd ~ /bootc install/) {
        inst++
        if (l[i-1] ~ /set_nvram_args_for/ && cmd ~ /NVRAM_ARGS/ && cmd ~ /NVRAM_BOOTC_ARGS/) good++
        else printf "UNGUARDED-INSTALL:%d ", i
      } else if (cmd ~ /LUKS_ENROLL_PATH/) {
        enrol++
      } else {
        printf "UNREVIEWED-PRIVILEGED:%d ", i
      }
    }
    printf "sites=%d install=%d guarded=%d enrol=%d\n", site, inst, good, enrol
  }' "$ENGINE")
nvsites=$(printf '%s' "$nvscan"  | sed -n 's/.*sites=\([0-9]*\).*/\1/p')
nvinst=$(printf '%s' "$nvscan"   | sed -n 's/.*install=\([0-9]*\).*/\1/p')
nvgood=$(printf '%s' "$nvscan"   | sed -n 's/.*guarded=\([0-9]*\).*/\1/p')
nvenrol=$(printf '%s' "$nvscan"  | sed -n 's/.*enrol=\([0-9]*\).*/\1/p')
case "$nvscan" in
    *UNGUARDED-INSTALL*|*UNREVIEWED-PRIVILEGED*)
        bad "every privileged install call site is guarded" "$nvscan" ;;
    *)
        if [ "${nvinst:-0}" -ge 4 ] && [ "${nvinst:-0}" = "${nvgood:-0}" ] \
           && [ "${nvenrol:-0}" = 1 ] \
           && [ "$(( ${nvinst:-0} + ${nvenrol:-0} ))" = "${nvsites:-0}" ]; then
            ok "every privileged install call site is guarded" "$nvscan"
        else
            bad "every privileged install call site is guarded" "$nvscan"
        fi ;;
esac

# MUTATION for the structural scan itself: take the guard off ONE install call
# site and the scan must name that site. Without this the scan could be a
# tautology that passes whatever the engine looks like.
MUT_SITE="$WORK/mutant-site"
cp "$ENGINE" "$MUT_SITE"
python3 - "$MUT_SITE" <<'PY' 2>/dev/null || sed -i '0,/^  set_nvram_args_for "\$DISK"$/{/^  set_nvram_args_for "\$DISK"$/d}' "$MUT_SITE"
import sys
p = sys.argv[1]
lines = open(p, encoding="utf-8").read().split("\n")
for i, l in enumerate(lines):
    if l.strip() == 'set_nvram_args_for "$DISK"':
        del lines[i]
        break
else:
    sys.exit(1)
open(p, "w", encoding="utf-8").write("\n".join(lines))
PY
if cmp -s "$ENGINE" "$MUT_SITE"; then
    bad "mutant: one call site loses its guard" "the mutation changed nothing"
elif ! bash -n "$MUT_SITE" 2>/dev/null; then
    bad "mutant: one call site loses its guard" "the mutant does not parse"
else
    mutscan=$(awk '
      { l[NR] = $0 }
      END {
        for (i = 1; i <= NR; i++) {
          if (l[i] !~ /run --rm --privileged/) continue
          cmd = l[i]; j = i
          while (l[j] ~ /\\$/ && j < NR) { j++; cmd = cmd " " l[j] }
          if (cmd ~ /bootc install/ && !(l[i-1] ~ /set_nvram_args_for/ && cmd ~ /NVRAM_ARGS/ && cmd ~ /NVRAM_BOOTC_ARGS/))
            printf "UNGUARDED-INSTALL:%d ", i
        }
      }' "$MUT_SITE")
    case "$mutscan" in
        *UNGUARDED-INSTALL*) ok "mutant: one call site loses its guard" "scan named it: $mutscan" ;;
        *) bad "mutant: one call site loses its guard" "the scan saw nothing wrong" ;;
    esac
fi

# MUTATION. Remove the one line that adds --generic-image and the loop case
# must stop skipping the firmware. One line, deleted by an exact match, so the
# mutant still parses.
MUT_NV="$WORK/mutant-nvram"
cp "$ENGINE" "$MUT_NV"
sed -i '/^    NVRAM_BOOTC_ARGS=(--generic-image)$/d' "$MUT_NV"
if cmp -s "$ENGINE" "$MUT_NV"; then
    bad "mutant: the firmware-skip flag" "the mutation changed nothing — the sed program matched no line"
elif ! bash -n "$MUT_NV" 2>/dev/null; then
    bad "mutant: the firmware-skip flag" "the mutant does not parse; the mutation ate more than its line"
else
    got=$(nvram_probe "$MUT_NV" /dev/loop9 "$FAKESYS")
    case "$got" in
        *"--generic-image"*) bad "mutant: the firmware-skip flag" "it survived its own deletion — the case proves nothing" ;;
        *)                   ok "mutant: the firmware-skip flag" "removed -> a loopback target would write NVRAM again" ;;
    esac
fi

# ── the keymap half, run where the data it reads actually exists ───────────
# These assertions need Fedora's keymap tree (/usr/lib/kbd/keymaps), systemd's
# kbd-model-map and xkeyboard-config's rules/base.lst. A GitHub ubuntu-24.04
# runner has none of them in that shape — Debian puts keymaps elsewhere and
# names them .kmap.gz — and while this block lived inline it SKIPped there. A
# skip is not a weaker pass: it meant the required deliverable of this round
# was measured on one developer's laptop and nowhere else, while the CI step
# still reported success. That is the gate-that-inspects-nothing shape this
# repository keeps finding.
#
# So: run installer/keymap-checks.sh here when this machine can answer it,
# otherwise run the identical script inside a Fedora container, and FAIL if
# neither route is available. It never skips.
KM_OUT="$WORK/keymap-out.txt"
if [ -z "${APEX_KEYMAP_FORCE_CONTAINER:-}" ] \
   && [ -f /usr/share/systemd/kbd-model-map ] && [ -d /usr/lib/kbd/keymaps ] \
   && [ -r /usr/share/X11/xkb/rules/base.lst ]; then
    bash ./keymap-checks.sh "$ENGINE" "$WORK" 2>&1 | tee "$KM_OUT"
elif CRT=$(command -v podman || command -v docker); then
    echo "note: no Fedora keymap data on this machine — measuring inside a container ($CRT)"
    # Fully qualified on purpose: a bare `fedora:43` makes podman ask which
    # registry it meant and makes the step fail for a reason that has nothing
    # to do with keymaps.
    "$CRT" run --rm -v "$PWD":/w:ro,z -w /w "${APEX_KEYMAP_IMAGE:-quay.io/fedora/fedora:43}" bash -c '
        dnf -y install --setopt=install_weak_deps=False kbd systemd xkeyboard-config >/dev/null 2>&1 || {
            echo "FAIL  the container could not install kbd, systemd and xkeyboard-config"; exit 1; }
        bash ./keymap-checks.sh ./apex-install /tmp/kmwork' 2>&1 | tee "$KM_OUT"
else
    echo "FAIL  the keymap conversion could not be measured here: no Fedora keymap data and no container runtime"
    printf 'KEYMAP-CHECKS: 0 1\n' > "$KM_OUT"
fi
# Fold the child's counts in. A MISSING result line is a failure, not a zero:
# a script that died halfway through must not read as "nothing to report".
kmline=$(grep -m1 '^KEYMAP-CHECKS: ' "$KM_OUT" 2>/dev/null || true)
if [ -z "$kmline" ]; then
    bad "the keymap checks reported a result" "no KEYMAP-CHECKS line — they did not finish"
else
    read -r _kmtag kp kf <<<"$kmline"
    : "$_kmtag"
    pass=$((pass + kp)); fail=$((fail + kf))
    # And a floor, so a future edit that quietly guts keymap-checks.sh cannot
    # turn 18 assertions into 0 and still report a clean run.
    if [ "$kp" -lt 15 ]; then
        bad "the keymap checks asserted their full set" "only $kp assertions ran"
    fi
fi

echo
echo "── --check-passphrase: the encrypt-page check, run standalone ─────────"
# The GUI calls this from the encrypt page's Continue handler, BEFORE the
# confirm step, so a Croatian owner whose passphrase has an `x` in it finds
# out while they can still pick a different one — not on the PROGRESS page,
# after the point of no return. It needs no image, no answers file, no disk:
# it is dispatched in the engine's own argument case, before any of that is
# read. `sudo -n true` is asserted directly rather than trusted, since this
# section has no ENGINE_RUNNABLE guard to fall back on.
if sudo -n true 2>/dev/null && [ -x "$ENGINE" ]; then
    out=$(printf '%s' "apexbootproof1" | sudo -n "$ENGINE" --check-passphrase us "" 2>&1); rc=$?
    if [ "$out" = "typeable: yes console=us" ] && [ "$rc" = 0 ]; then
        ok "us + an all-ASCII passphrase: typeable: yes"
    else bad "us + an all-ASCII passphrase: typeable: yes" "rc=$rc out='$out'"; fi

    # vn is on the record (installer/keymap-checks.sh's own measured table,
    # also asserted above) as having no digit 1 anywhere in its console
    # keymap — the same fact the real install path would discover and fall
    # back to `us` for, just asked for directly instead of via a live install.
    out=$(printf '%s' "apex1zed" | sudo -n "$ENGINE" --check-passphrase vn "" 2>&1); rc=$?
    if [ "$out" = "typeable: no console=vn chars=1" ] && [ "$rc" = 0 ]; then
        ok "vn + a passphrase containing '1': typeable: no, names the character"
    else bad "vn + a passphrase containing '1': typeable: no, names the character" "rc=$rc out='$out'"; fi

    # No layout at all must not crash the GUI's call — it is asked before the
    # user has necessarily reached the keyboard page in every flow.
    out=$(printf '%s' "hello" | sudo -n "$ENGINE" --check-passphrase "" "" 2>&1); rc=$?
    if [ "$out" = "typeable: unknown reason=no-layout" ] && [ "$rc" = 0 ]; then
        ok "no layout: typeable: unknown, not a crash"
    else bad "no layout: typeable: unknown, not a crash" "rc=$rc out='$out'"; fi

    # THE PROPERTY THAT MATTERS MOST: this is advice for a form field, never a
    # gate. A verdict of "no" must still exit 0 — a future edit that turns this
    # into `exit 1` on "no" would make the GUI's subprocess call read a
    # typeable-but-inconvenient passphrase as "the engine crashed" and block an
    # install this same passphrase would succeed at today.
    printf '%s' "apex1zed" | sudo -n "$ENGINE" --check-passphrase vn "" >/dev/null 2>&1
    _cprc=$?
    if [ "$_cprc" = 0 ]; then ok "a 'no' verdict still exits 0 — advisory, never a gate"
    else bad "a 'no' verdict still exits 0 — advisory, never a gate" "exit $_cprc"; fi

    # MUTATION: remove the case arm entirely and confirm the same call falls
    # through to the engine's ordinary refusal instead of silently doing
    # nothing — a copy of the engine, the original is never touched.
    CPMUT="$WORK/apex-install.check-passphrase-mutant"
    sed '/^  --check-passphrase)$/,/^    ;;$/d' "$ENGINE" > "$CPMUT" 2>/dev/null
    chmod 755 "$CPMUT" 2>/dev/null
    # A substring match on --check-passphrase would also match the header
    # comment and the die() message naming it — both mention the flag by name
    # and neither is what this mutation removes. The CASE LABEL is the thing
    # under test.
    if grep -q -- '^  --check-passphrase)$' "$CPMUT"; then
        bad "mutant: the case arm removed" "the sed program matched no line — the mutant is identical to the engine"
    else
        mutout=$(printf '%s' "apexbootproof1" | sudo -n "$CPMUT" --check-passphrase us "" 2>&1)
        if [[ "$mutout" == *"unknown argument"* ]]; then
            ok "mutant: the case arm removed" "falls through to the ordinary refusal, as it must"
        else
            bad "mutant: the case arm removed" "got '$mutout' — something still answers --check-passphrase with the arm gone"
        fi
    fi
else
    echo "SKIP  --check-passphrase cases (need passwordless sudo and the engine present)"
fi

echo
echo "──────────────────────────────────────────────────────────────────────"
printf '%s passed, %s failed\n' "$pass" "$fail"
[ "$fail" = 0 ] || exit 1
[ "$pass" -gt 0 ] || { echo "FATAL: nothing was asserted"; exit 1; }
exit 0
