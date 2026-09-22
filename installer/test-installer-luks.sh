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

# ── the keymap data the engine's conversion reads, AS A FIXTURE ─────────────
#
# WHY. The assertions further down run the SHIPPED ENGINE and read back the
# console keymap it resolved. Until now they read it out of whatever kbd
# package the machine running this suite happened to have — and a GitHub
# ubuntu-24.04 runner has no /usr/lib/kbd/keymaps and no
# /usr/share/systemd/kbd-model-map in the shape the engine expects. So every
# layout resolved to the `us` fallback, three assertions were red, and this
# suite had NEVER ONCE PASSED in CI since it landed on 2026-09-20:
#
#   FAIL XKB bg resolves to console keymap bg_bds-utf8 (end to end)  KEYMAP=us
#   FAIL vn + a passphrase containing '1'  out='typeable: yes console=us'
#
# That is the same hermeticity defect test-installer-locale.sh was fixed for
# (25/1 -> 26/0): a suite whose verdict is a property of the tester's machine
# rather than of the code under test.
#
# WHAT MOVES INTO THE FIXTURE AND WHAT DOES NOT. The claim "Fedora's kbd really
# does ship bg_bds-utf8, and `vn` really has no digit 1 anywhere in its table"
# is REAL DATA and stays where it belongs: installer/keymap-checks.sh asserts
# it over all 99 XKB layouts and all 562 keymaps, on a Fedora host or inside
# quay.io/fedora/fedora:43. What the fixture makes hermetic is the ENGINE
# WIRING — that an operator's `keymap=` reaches console_keymap_for(), that its
# answer reaches the install summary and the --check-passphrase verdict, and
# that an untypeable character is NAMED rather than merely counted. That wiring
# has to give the same answer on every machine, and now does.
#
# The engine reads the fixture through APEX_KBD_KEYMAPS / APEX_KBD_MODEL_MAP,
# the same shape of testing hook as the APEX_IMAGE the rest of this file
# already uses. Nothing the GUI, the answers file or the kernel command line
# can reach sets them.
#
# PROVED CONSULTED, NOT ASSUMED. Every fixture-backed assertion below is paired
# with a control that re-runs the identical call against an EMPTY tree and
# requires the `us` fallback back — which is the CI red reproduced on purpose.
# Without that pair a fixture that was silently ignored would look like a pass.
KBD_TREE="$WORK/kbdfix/keymaps"
KBD_MODELMAP="$WORK/kbdfix/kbd-model-map"
KBD_NONE="$WORK/kbdnone"
mkdir -p "$KBD_TREE/xkb" "$KBD_NONE"

# Shaped like the xkb-converted maps kbd ships: one `keycode N = ...` line per
# key, with keysym NAMES for the digits and punctuation. The names are not
# decoration — they are what makes the engine's own name table load-bearing,
# and a fixture written with bare characters would leave that table untested.
{
    printf 'keymaps 0-2,4-5,8,12\n'
    printf 'keycode %3d = %s %s\n' \
        16 q Q  17 w W  18 e E  19 r R  20 t T  21 y Y  22 u U  23 i I \
        24 o O  25 p P  30 a A  31 s S  32 d D  33 f F  34 g G  35 h H \
        36 j J  37 k K  38 l L  44 z Z  45 x X  46 c C  47 v V  48 b B \
        49 n N  50 m M
    printf 'keycode %3d = %s\n' \
        3 'two at'            4 'three numbersign'  5 'four dollar' \
        6 'five percent'      7 'six asciicircum'   8 'seven ampersand' \
        9 'eight asterisk'   10 'nine parenleft'   11 'zero parenright' \
        57 'space'
} > "$KBD_TREE/xkb/vn.map"
# The `1` key is the one thing this map does NOT have — the hole the real `vn`
# has, and the whole point of the vn case below.

# bg_bds-utf8 is the same map plus the digit 1, and it is GZIPPED: the engine's
# _keymap_cat() has a zcat branch for exactly the form Fedora ships, and a
# fixture of plain files would never enter it.
{ cat "$KBD_TREE/xkb/vn.map"; printf 'keycode %3d = one exclam\n' 2; } \
    > "$KBD_TREE/xkb/bg_bds-utf8.map"
gzip -n "$KBD_TREE/xkb/bg_bds-utf8.map" 2>/dev/null || true

# Both real `bg` rows, copied verbatim from /usr/share/systemd/kbd-model-map,
# in the real file's order. Two rows, not one, and that is deliberate:
#   * `bg,us` is a LIST, so it only matches through the engine's
#     `index($2, l ",")==1` clause — the clause whose absence is what sent bg
#     to `us` with bg_bds-utf8.map.gz sitting unused in the tree;
#   * bg_pho-utf8 comes FIRST and carries a variant, so an engine that took
#     the first row for the layout regardless of variant would answer
#     bg_pho-utf8 — for which this fixture deliberately ships NO keymap file,
#     so the engine's step-4 "does the table's answer actually exist" check
#     would then drop it to `us` and the assertion would go red.
printf '%s\n' \
    '# fixture — two rows copied from /usr/share/systemd/kbd-model-map' \
    'bg_pho-utf8\tbg,us\tpc105\t,phonetic\tterminate:ctrl_alt_bksp,grp:shifts_toggle' \
    'bg_bds-utf8\tbg,us\tpc105\t-\tterminate:ctrl_alt_bksp,grp:shifts_toggle' \
    | sed 's/\\t/\t/g' > "$KBD_MODELMAP"

for _f in "$KBD_TREE/xkb/vn.map" "$KBD_MODELMAP"; do
    [ -s "$_f" ] || { echo "FATAL: the keymap fixture was not built: $_f"; exit 1; }
done

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

    # dry_run <engine> <keymap-tree> <model-map> — the same run three ways.
    dry_run() {
        sudo -n APEX_IMAGE="$ENGINE_IMAGE" APEX_DRY_RUN=1 APEX_LUKS_ENROLL_LOCAL="$HELPER" \
                APEX_KBD_KEYMAPS="$2" APEX_KBD_MODEL_MAP="$3" \
                "$1" --headless "$ANS" 2>&1 </dev/null
    }
    # The summary line's KEYMAP=, or the empty string. Never `grep -q` in a
    # pipeline here: pipefail turns a match into 141 via SIGPIPE on the writer.
    dry_keymap() { printf '%s' "$1" | grep -o 'KEYMAP=[a-z0-9_.-]*' | tail -1; }

    out=$(dry_run "$ENGINE" "$KBD_TREE" "$KBD_MODELMAP")
    if [[ "$out" == *"APEX-INSTALL-DRYRUN-OK"* ]]; then ok "encrypt=yes reaches the dry-run stop"
    else bad "encrypt=yes reaches the dry-run stop" "$(printf '%s' "$out" | tail -3 | tr '\n' ' ')"; fi
    if [[ "$out" == *"ENCRYPT=yes"* ]]; then ok "the dry run names the encryption decision"
    else bad "the dry run names the encryption decision" "no ENCRYPT= in the summary"; fi
    if [ "$(dry_keymap "$out")" = "KEYMAP=bg_bds-utf8" ]; then
        ok "XKB bg resolves to console keymap bg_bds-utf8 (end to end)"
    else
        bad "XKB bg resolves to console keymap bg_bds-utf8 (end to end)" \
            "$(dry_keymap "$out")"
    fi

    # CONTROL, and the CI red reproduced deliberately: point the engine at an
    # EMPTY tree and the same answers file must come back `us`. If this ever
    # passes AND the line above passes, the engine is reading keymap data this
    # suite did not put there and the assertion above is measuring the machine.
    out_none=$(dry_run "$ENGINE" "$KBD_NONE" "$KBD_NONE/absent-model-map")
    if [ "$(dry_keymap "$out_none")" = "KEYMAP=us" ]; then
        ok "…and an empty keymap tree falls back to us" "so the line above read the fixture, not this machine"
    else
        bad "…and an empty keymap tree falls back to us" \
            "got '$(dry_keymap "$out_none")' — the fixture is not what the engine consulted"
    fi

    # MUTATION on the ENGINE, not on the fixture. `bg` is written `bg,us` in
    # systemd's table; a plain `$2 == layout` test matches no row of that shape,
    # and that is precisely how bg used to fall through to `us` while
    # bg_bds-utf8.map.gz sat unused in the keymap tree. Delete the clause that
    # makes a list match and the identical run must report `us`.
    KMMUT="$WORK/apex-install.multilayout-mutant"
    sed 's/index($2, l ",")==1/0/g' "$ENGINE" > "$KMMUT"
    chmod 755 "$KMMUT"
    if cmp -s "$ENGINE" "$KMMUT"; then
        bad "mutant: the multi-layout table row" \
            "the sed program matched no line — the mutant is the engine unchanged"
    else
        mout=$(dry_run "$KMMUT" "$KBD_TREE" "$KBD_MODELMAP")
        if [ "$(dry_keymap "$mout")" = "KEYMAP=us" ]; then
            ok "mutant: the multi-layout table row" "clause removed -> bg falls back to us"
        else
            bad "mutant: the multi-layout table row" \
                "got '$(dry_keymap "$mout")' with the clause gone — the assertion above proves nothing"
        fi
    fi
    rm -f "$KMMUT"
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

# km_runtime — sets KM_RT to a container runtime that actually STARTS here.
#
# ROOT podman first, and not as a formality. On a GitHub runner there is no
# systemd user session, so rootless podman tries to create its run directory
# under /run/user/$UID and dies:
#
#   cannot open run directory '/run/user/1001/crun': Permission denied
#   Error: OCI permission denied
#
# The container never started, keymap-checks.sh never printed its result line,
# and the caller's "did they report anything" guard fired as
# `FAIL the keymap checks reported a result` — the third of this suite's three
# permanent CI reds. Root podman uses /run/podman and needs no such directory,
# and this suite has ALREADY proved `sudo -n podman` works in this process: it
# is how the engine image above was made.
#
# Each candidate is PROBED with `info`, not merely found on PATH. `command -v`
# answering is what made the old chooser pick a podman that could not run a
# container, and a runtime that cannot start is indistinguishable from one that
# is absent as far as this measurement is concerned.
KM_RT=()
km_runtime() {
    if command -v podman >/dev/null 2>&1 && sudo -n podman info >/dev/null 2>&1; then
        KM_RT=(sudo -n podman); return 0
    fi
    if command -v podman >/dev/null 2>&1 && podman info >/dev/null 2>&1; then
        KM_RT=(podman); return 0
    fi
    if command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then
        KM_RT=(docker); return 0
    fi
    return 1
}

if [ -z "${APEX_KEYMAP_FORCE_CONTAINER:-}" ] \
   && [ -f /usr/share/systemd/kbd-model-map ] && [ -d /usr/lib/kbd/keymaps ] \
   && [ -r /usr/share/X11/xkb/rules/base.lst ]; then
    bash ./keymap-checks.sh "$ENGINE" "$WORK" 2>&1 | tee "$KM_OUT"
elif km_runtime; then
    echo "note: no Fedora keymap data on this machine — measuring inside a container (${KM_RT[*]})"
    # Fully qualified on purpose: a bare `fedora:43` makes podman ask which
    # registry it meant and makes the step fail for a reason that has nothing
    # to do with keymaps.
    "${KM_RT[@]}" run --rm -v "$PWD":/w:ro,z -w /w "${APEX_KEYMAP_IMAGE:-quay.io/fedora/fedora:43}" bash -c '
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
    # cp_check <engine> <passphrase> <layout> <keymap-tree> — one verdict line.
    # The fixture tree is passed on every call for the reason given where it is
    # built: without it these six assertions read the tester's own kbd package
    # and were red on every CI runner this suite has ever run on.
    cp_check() {
        printf '%s' "$2" | sudo -n APEX_KBD_KEYMAPS="$4" APEX_KBD_MODEL_MAP="$KBD_MODELMAP" \
            "$1" --check-passphrase "$3" "" 2>&1
    }

    out=$(cp_check "$ENGINE" "apexbootproof1" us "$KBD_TREE"); rc=$?
    if [ "$out" = "typeable: yes console=us" ] && [ "$rc" = 0 ]; then
        ok "us + an all-ASCII passphrase: typeable: yes"
    else bad "us + an all-ASCII passphrase: typeable: yes" "rc=$rc out='$out'"; fi

    # THE CASE THIS MODE EXISTS FOR. A keymap with no digit `1` anywhere in its
    # table must come back `no`, naming the character — not a count, not a
    # shrug. `vn` is that keymap on a real Fedora tree, which
    # installer/keymap-checks.sh asserts against the real file; the fixture
    # here reproduces the SHAPE so that the engine's wiring — dispatch ->
    # console_keymap_for -> keymap_can_type -> this exact output format — is
    # measured identically on a machine with no Fedora kbd at all.
    out=$(cp_check "$ENGINE" "apex1zed" vn "$KBD_TREE"); rc=$?
    if [ "$out" = "typeable: no console=vn chars=1" ] && [ "$rc" = 0 ]; then
        ok "vn + a passphrase containing '1': typeable: no, names the character"
    else bad "vn + a passphrase containing '1': typeable: no, names the character" "rc=$rc out='$out'"; fi

    # CONTROL, and the CI red reproduced: with an EMPTY tree the layout cannot
    # resolve, the engine falls back to `us`, and the verdict is the useless
    # `typeable: yes console=us` this suite used to report as a pass. If this
    # and the case above are ever both green, the fixture is being ignored.
    out=$(cp_check "$ENGINE" "apex1zed" vn "$KBD_NONE"); rc=$?
    if [ "$out" = "typeable: yes console=us" ] && [ "$rc" = 0 ]; then
        ok "…and with an empty keymap tree it falls back to us" "so the case above read the fixture"
    else bad "…and with an empty keymap tree it falls back to us" "rc=$rc out='$out'"; fi

    # THE COUNTER-HALF. A verdict of `no` proves nothing if the check says `no`
    # to everything. bg reaches its keymap only THROUGH the conversion table's
    # `bg,us` row, so this one assertion covers both halves at once: the
    # multi-layout lookup, and a passphrase the resulting keymap can type.
    out=$(cp_check "$ENGINE" "correct horse 9" bg "$KBD_TREE"); rc=$?
    if [ "$out" = "typeable: yes console=bg_bds-utf8" ] && [ "$rc" = 0 ]; then
        ok "bg + a passphrase it can type: typeable: yes, on the converted name"
    else bad "bg + a passphrase it can type: typeable: yes, on the converted name" "rc=$rc out='$out'"; fi

    # MUTATION: make keymap_can_type() unable to ever report a missing
    # character. The vn case above must then go green-for-the-wrong-reason,
    # which is what this arm refuses to let happen silently.
    KCMUT="$WORK/apex-install.can-type-mutant"
    sed 's/\[ -n "$missing" \]/[ -z "$missing" ]/' "$ENGINE" > "$KCMUT"
    chmod 755 "$KCMUT"
    if cmp -s "$ENGINE" "$KCMUT"; then
        bad "mutant: keymap_can_type can no longer say no" \
            "the sed program matched no line — the mutant is the engine unchanged"
    else
        mout=$(cp_check "$KCMUT" "apex1zed" vn "$KBD_TREE")
        if [ "$mout" = "typeable: no console=vn chars=1" ]; then
            bad "mutant: keymap_can_type can no longer say no" \
                "it still reported the missing character with the branch removed"
        else
            ok "mutant: keymap_can_type can no longer say no" "verdict removed -> '$mout'"
        fi
    fi
    rm -f "$KCMUT"

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
    printf '%s' "apex1zed" \
        | sudo -n APEX_KBD_KEYMAPS="$KBD_TREE" APEX_KBD_MODEL_MAP="$KBD_MODELMAP" \
            "$ENGINE" --check-passphrase vn "" >/dev/null 2>&1
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
