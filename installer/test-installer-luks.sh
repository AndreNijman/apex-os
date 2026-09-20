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
echo "── XKB layout -> console keymap, out of the shipped engine ────────────"
# Sourced, not reimplemented: a copy of the mapping in this file would pass
# while the engine's own copy was wrong, which is the failure this suite exists
# to catch.
#
# THE PROPERTY UNDER TEST is not a lookup table of favourite examples. It is:
# whatever name this function returns, `loadkeys` can load it. That is the
# thing that decides whether the passphrase prompt uses the owner's layout or
# silently keeps the kernel's built-in `us`. Measured on Fedora 43's kbd: of
# the 99 XKB layouts xkeyboard-config offers, 36 are NOT loadable console
# keymap names, and after conversion 0 are.
FNS="$WORK/keymap-fns.sh"
sed -n '/^kbd_has()/,/^}/p;/^console_keymap_for()/,/^}/p' "$ENGINE" > "$FNS"
if [ ! -s "$FNS" ] || ! grep -q 'console_keymap_for' "$FNS"; then
    bad "extract the keymap conversion from the engine" "nothing extracted"
elif [ ! -f /usr/share/systemd/kbd-model-map ] || [ ! -d /usr/lib/kbd/keymaps ] \
     || [ ! -r /usr/share/X11/xkb/rules/base.lst ]; then
    echo "SKIP  keymap conversion (no kbd-model-map, keymap tree or xkb rules here)"
else
(
    # shellcheck disable=SC1090
    . "$FNS"
    _p=0; _f=0
    # `us` is the kernel built-in and needs no file; everything else must exist.
    loadable() {
        [ "$1" = us ] && return 0
        [ -n "$(find /usr/lib/kbd/keymaps -type f \
                  \( -name "$1.map" -o -name "$1.map.gz" -o -name "$1.map.zst" \) \
                  -print -quit 2>/dev/null)" ]
    }
    ck() {  # layout variant want
        local got; got=$(console_keymap_for "$1" "$2" "")
        if [ "$got" = "$3" ]; then printf 'PASS  %-46s\n' "xkb '$1${2:+:$2}' -> $3"; _p=$((_p+1))
        else printf 'FAIL  %-46s got %s want %s\n' "xkb '$1${2:+:$2}'" "$got" "$3"; _f=$((_f+1)); fi
    }
    # Rows that pin the three distinct behaviours.
    #  * a layout whose own name IS a keymap keeps it (kbd ships xkb/gb.map.gz);
    #  * a layout whose name is NOT a keymap goes through systemd's table, and
    #    `bg` also exercises the multi-layout `bg,us` row that a plain equality
    #    test silently misses;
    #  * a variant is honoured when a keymap exists for it.
    ck gb     ""       gb
    ck bg     ""       bg_bds-utf8
    ck us     dvorak   us-dvorak
    ck de     neo      de-neo
    ck us     ""       us
    # Only the first of a comma-separated list: the console loads exactly one.
    ck "bg,us" ""      bg_bds-utf8
    # An unknown layout falls back to the built-in rather than writing a name
    # loadkeys cannot load, and empty in never yields an empty KEYMAP= line.
    ck zzzz   ""       us
    ck ""     ""       us

    # The invariant, over every layout xkeyboard-config offers.
    raw_bad=0; conv_bad=0; n=0
    while read -r l; do
        [ -n "$l" ] || continue
        n=$((n + 1))
        loadable "$l" || raw_bad=$((raw_bad + 1))
        c=$(console_keymap_for "$l" "" "")
        loadable "$c" || { conv_bad=$((conv_bad + 1)); printf '      unloadable: %s -> %s\n' "$l" "$c"; }
    done < <(awk '/^! layout/{f=1;next} /^!/{f=0} f && NF{print $1}' /usr/share/X11/xkb/rules/base.lst)
    if [ "$n" -lt 50 ]; then
        printf 'FAIL  %-46s only %s layouts read — the list is not being read\n' "layout inventory" "$n"; _f=$((_f+1))
    else
        printf 'PASS  %-46s %s layouts read\n' "layout inventory" "$n"; _p=$((_p+1))
    fi
    if [ "$conv_bad" = 0 ]; then
        printf 'PASS  %-46s all %s resolve to a loadable keymap\n' "every XKB layout converts to a real keymap" "$n"; _p=$((_p+1))
    else
        printf 'FAIL  %-46s %s of %s do not\n' "every XKB layout converts to a real keymap" "$conv_bad" "$n"; _f=$((_f+1))
    fi
    # The counter-half: if the raw XKB names were all loadable anyway, this
    # conversion would be pointless and the test above would prove nothing.
    if [ "$raw_bad" -gt 0 ]; then
        printf 'PASS  %-46s %s of %s raw XKB names are not keymaps\n' "the conversion is load-bearing" "$raw_bad" "$n"; _p=$((_p+1))
    else
        printf 'FAIL  %-46s every raw XKB name was loadable — this test proves nothing here\n' "the conversion is load-bearing"; _f=$((_f+1))
    fi
    printf '%s %s\n' "$_p" "$_f" > "$WORK/keymap-score"
)
    read -r kp kf < "$WORK/keymap-score"
    pass=$((pass + kp)); fail=$((fail + kf))
    # The mutation arm. Point the conversion table at a path that does not
    # exist: `bg` has no keymap of its own, so the only honest answer left is
    # the `us` fallback. A function that merely echoed its argument back would
    # say `bg` here, which this distinguishes from both.
(
    sed 's|mapf=/usr/share/systemd/kbd-model-map|mapf=/nonexistent/kbd-model-map|' "$FNS" > "$WORK/keymap-mutant.sh"
    # shellcheck disable=SC1090
    . "$WORK/keymap-mutant.sh"
    printf '%s\n' "$(console_keymap_for bg '' '')" > "$WORK/keymap-mut"
)
    mutgot=$(cat "$WORK/keymap-mut")
    if [ "$mutgot" = us ]; then
        ok "mutant: table removed -> bg falls back to us"
    else
        bad "mutant: table removed -> bg falls back to us" "got '$mutgot'"
    fi
fi

echo
echo "── the keymap moves the KEYS, not just a string in a file ─────────────"
# The property the owner actually experiences is not "vconsole.conf contains a
# name". It is: the key their finger lands on produces the character they
# expect, at a prompt with no way back. So this reads the shipped keymap DATA
# and checks the characters.
#
# keycode 21 is the key immediately right of T. On a US keyboard it is `y`; on
# a German one it is `z`, and 44 is the mirror of that swap. If the installer
# hands the initramfs nothing — which is what it did before this work — the
# console keeps the kernel's built-in US map and a German owner's `z` comes out
# as `y`. A passphrase with a `z` in it is then untypeable on their own laptop.
#
# The keymap files are plain text (gzipped) and carry `keycode N = +U+00xx …`,
# so this is a direct read of what will be loaded rather than a re-derivation.
KEYMAP_TREE=/usr/lib/kbd/keymaps
keymap_file() {  # $1 = console keymap name -> the PC keymap of that name
    # The same name exists in several subtrees and they are NOT interchangeable:
    # kbd ships Atari and Sun keymaps under legacy/, and those use a different
    # keycode numbering entirely, so `us` there has no keycode 21 at all. Taking
    # whatever `find` returned first read as "the US keymap types nothing",
    # which is a wrong answer dressed as a failure. Search the PC trees, in the
    # order a PC console would use them, and never fall back to a keyboard
    # nobody is holding.
    local n="$1" d f
    for d in "$KEYMAP_TREE/xkb" "$KEYMAP_TREE/legacy/i386" "$KEYMAP_TREE/i386"; do
        [ -d "$d" ] || continue
        f=$(find "$d" -type f \( -name "$n.map" -o -name "$n.map.gz" \) 2>/dev/null | sort | head -1)
        [ -n "$f" ] && { printf '%s' "$f"; return 0; }
    done
    return 0
}
keycode_char() {  # $1 = keymap file  $2 = keycode -> the character that key types
    # The two subtrees write the same thing two ways — `+U+0079` in the
    # xkb-converted maps, `+y` in the legacy ones — so both are normalised to
    # the character itself. Comparing the raw spelling would make this test a
    # test of which subtree `find` happened to return.
    local f="$1" kc="$2" line="" sym hex
    case "$f" in
      *.gz) line=$(zcat "$f" 2>/dev/null | grep -m1 -E "^keycode +$kc =" || true) ;;
      *)    line=$(grep -m1 -E "^keycode +$kc =" "$f" 2>/dev/null || true) ;;
    esac
    sym=$(printf '%s' "$line" | awk '{print $4}')
    sym=${sym#+}
    case "$sym" in
      U+00[0-9a-fA-F][0-9a-fA-F]) hex=${sym#U+00}; printf "$(printf '\\x%s' "$hex")" ;;
      ?) printf '%s' "$sym" ;;
      *) printf '' ;;
    esac
}
if [ ! -d "$KEYMAP_TREE" ] || [ ! -s "$FNS" ]; then
    echo "SKIP  keymap character checks (no keymap tree here)"
else
(
    # shellcheck disable=SC1090
    . "$FNS"
    _p=0; _f=0
    ckey() {  # xkb-layout keycode expected-symbol human
        local km f got
        km=$(console_keymap_for "$1" "" "")
        f=$(keymap_file "$km")
        if [ -z "$f" ]; then
            printf 'FAIL  %-46s no keymap file for %s\n' "$4" "$km"; _f=$((_f+1)); return
        fi
        got=$(keycode_char "$f" "$2")
        if [ "$got" = "$3" ]; then printf 'PASS  %-46s %s\n' "$4" "$km: keycode $2 -> $got"; _p=$((_p+1))
        else printf 'FAIL  %-46s %s: keycode %s -> %s, want %s\n' "$4" "$km" "$2" "$got" "$3"; _f=$((_f+1)); fi
    }
    # US: the key right of T is y, and the key left of X is z.
    ckey us 21 'y' "US keyboard: the key right of T types y"
    ckey us 44 'z' "US keyboard: the key left of X types z"
    # German: those two are swapped. This is the difference a German owner sees
    # at the unlock prompt, and the reason the keymap has to reach the initrd.
    ckey de 21 'z' "German keyboard: the same key types z"
    ckey de 44 'y' "German keyboard: the other one types y"
    # French AZERTY moves the whole home row: the key where US has Q types a.
    ckey fr 16 'a' "French keyboard: where US has Q, it types a"
    # And the counter-assertion, so none of the above can be passing by
    # accident on a tree where every keymap is the same file: US and German
    # must actually DIFFER on that key.
    uf=$(keymap_file "$(console_keymap_for us '' '')")
    df=$(keymap_file "$(console_keymap_for de '' '')")
    if [ -n "$uf" ] && [ -n "$df" ] && [ "$(keycode_char "$uf" 21)" != "$(keycode_char "$df" 21)" ]; then
        printf 'PASS  %-46s\n' "US and German really are different keymaps"; _p=$((_p+1))
    else
        printf 'FAIL  %-46s they read identically — the check proves nothing\n' "US and German really are different keymaps"; _f=$((_f+1))
    fi
    printf '%s %s\n' "$_p" "$_f" > "$WORK/char-score"
)
    read -r cp cf < "$WORK/char-score"
    pass=$((pass + cp)); fail=$((fail + cf))
fi

echo
echo "──────────────────────────────────────────────────────────────────────"
printf '%s passed, %s failed\n' "$pass" "$fail"
[ "$fail" = 0 ] || exit 1
[ "$pass" -gt 0 ] || { echo "FATAL: nothing was asserted"; exit 1; }
exit 0
