#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  keymap-checks.sh — the XKB-layout-to-console-keymap half of
#  test-installer-luks.sh, in its own file so it can be run somewhere the data
#  it needs actually exists.
#
#  WHY IT IS SEPARATE. These assertions read three things that only a Fedora
#  machine has in the shape the engine expects: /usr/lib/kbd/keymaps (Debian
#  and Ubuntu put keymaps elsewhere and name them .kmap.gz, not .map.gz),
#  /usr/share/systemd/kbd-model-map, and xkeyboard-config's rules/base.lst. On
#  a GitHub ubuntu-24.04 runner the keymap tree is absent, so when this lived
#  inline it SKIPped — and a skip here is not a weaker pass, it is the whole
#  required deliverable of this round going unmeasured while the step still
#  reports success. The caller now runs this inside fedora:43 when the local
#  machine cannot answer, and fails outright when neither route is available.
#
#  It never SKIPs. Missing data is a FAIL that names the package.
#
#  Usage: keymap-checks.sh <path-to-apex-install> <writable-work-dir>
#  Prints PASS/FAIL lines and, as its last line, `KEYMAP-CHECKS: <p> <f>`.
#  The caller folds those counts into its own and treats a missing last line
#  as a failure, so a crash in here cannot read as "nothing to report".
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail

ENGINE="${1:?usage: keymap-checks.sh <engine> <workdir>}"
WORK="${2:?usage: keymap-checks.sh <engine> <workdir>}"
mkdir -p "$WORK" || exit 1

pass=0; fail=0
ok()  { printf 'PASS  %-46s %s\n' "$1" "${2:-}"; pass=$((pass+1)); }
bad() { printf 'FAIL  %-46s %s\n' "$1" "${2:-}"; fail=$((fail+1)); }

MODELMAP=/usr/share/systemd/kbd-model-map
KEYMAP_TREE=/usr/lib/kbd/keymaps
XKBRULES=/usr/share/X11/xkb/rules/base.lst

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
[ -s "$FNS" ] && grep -q 'console_keymap_for' "$FNS" \
    || bad "extract the keymap conversion from the engine" "nothing extracted"

# The data gate. A FAIL, never a SKIP — see the header.
missing=""
[ -f "$MODELMAP" ]    || missing="$missing $MODELMAP(systemd)"
[ -d "$KEYMAP_TREE" ] || missing="$missing $KEYMAP_TREE(kbd)"
[ -r "$XKBRULES" ]    || missing="$missing $XKBRULES(xkeyboard-config)"
if [ -n "$missing" ]; then
    bad "the keymap data needed to measure this is present" "missing:$missing"
    printf 'KEYMAP-CHECKS: %s %s\n' "$pass" "$fail"
    exit 1
fi

if [ -s "$FNS" ]; then
(
    # shellcheck disable=SC1090
    . "$FNS"
    _p=0; _f=0
    # `us` is the kernel built-in and needs no file; everything else must exist.
    loadable() {
        [ "$1" = us ] && return 0
        [ -n "$(find "$KEYMAP_TREE" -type f \
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
    done < <(awk '/^! layout/{f=1;next} /^!/{f=0} f && NF{print $1}' "$XKBRULES")
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
    sed "s|mapf=$MODELMAP|mapf=/nonexistent/kbd-model-map|" "$FNS" > "$WORK/keymap-mutant.sh"
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
if [ -s "$FNS" ]; then
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

printf 'KEYMAP-CHECKS: %s %s\n' "$pass" "$fail"
[ "$fail" = 0 ] || exit 1
exit 0
