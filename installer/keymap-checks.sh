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

# `cmp` is NOT in the container this file runs in. MEASURED: inside
# quay.io/fedora/fedora:43 with kbd, systemd and xkeyboard-config installed,
# `cmp -s a b` prints "command not found" and exits 127, which reads as "the
# files differ" — so the guard that catches a mutation matching nothing was
# inert in exactly the environment CI uses. Compared with the shell instead.
# Defined here rather than beside its first user because BOTH mutation arms
# below need it, and the earlier one had no guard at all until 2026-09-22.
same_file() { [ "$(cat "$1")" = "$(cat "$2")" ]; }

# THIS FILE MEASURES REAL DATA, AND MUST NOT BE REDIRECTABLE.
# apex-install's console_keymap_for()/keymap_can_type() honour
# APEX_KBD_KEYMAPS and APEX_KBD_MODEL_MAP so that test-installer-luks.sh can
# measure the engine's WIRING against a fixture on a machine with no Fedora
# kbd. The division of labour only holds if this half still reads the real
# tree, so an ambient value of either — exported by a caller, or left in the
# environment — is dropped here rather than silently obeyed.
unset APEX_KBD_KEYMAPS APEX_KBD_MODEL_MAP

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
    #
    # Anchored on the PATH, not on the assignment. The first version of this
    # matched the literal `mapf=/usr/share/systemd/kbd-model-map`; when the
    # engine's default became an override-able `${APEX_KBD_MODEL_MAP:-...}`
    # the sed program matched nothing, the "mutant" was a byte-identical copy
    # of the function, and it answered `bg_bds-utf8` — a mutation arm that had
    # quietly stopped mutating. The guard below is why that was caught in one
    # run instead of becoming a permanently green test of nothing.
    sed "s|$MODELMAP|/nonexistent/kbd-model-map|g" "$FNS" > "$WORK/keymap-mutant.sh"
    if same_file "$FNS" "$WORK/keymap-mutant.sh"; then
        bad "mutant: table removed -> bg falls back to us" \
            "the sed program matched no line — the mutant is the function unchanged"
    else
(
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
fi

echo
echo
echo "── can the owner TYPE their passphrase on the layout they chose? ──────"
# The passphrase validation in the engine restricts the passphrase to printable
# ASCII, and its comment used to claim that every printable ASCII character is
# reachable on every kbd keymap. MEASURED across the whole shipped tree, that
# is false for 15 of 562 keymaps, and each one is a machine whose owner cannot
# type their own passphrase at the one prompt with no way back:
#
#   hr-unicode, rs-latinunicode, ba-unicode, me-latinunicode, epo-legacy
#       no q, w, x or y ANYWHERE in the table
#   vn, kz-latin, cm-azerty            no `1`
#   it-geo, ge-ergonomic               Georgian; several Latin letters absent
#   fa                                 Persian; almost no Latin at all
#
# These assertions run the engine's own functions against the real tree.

TFNS="$WORK/typeable-fns.sh"
sed -n '/^_keymap_cat()/,/^}/p;/^_keymap_text()/,/^}/p;/^keymap_ascii_set()/,/^}/p;/^keymap_can_type()/,/^}/p' "$ENGINE" > "$TFNS"
if ! grep -q 'keymap_can_type' "$TFNS"; then
    bad "extract the typeability check from the engine" "nothing extracted"
else
(
    # shellcheck disable=SC1090
    . "$TFNS"
    _p=0; _f=0
    T="$KEYMAP_TREE"
    tcase() {  # keymap passphrase expected-prefix human
        local got
        got=$(keymap_can_type "$2" "$T" "$1")
        case "$got" in
          "$3"*) printf 'PASS  %-46s %s -> %s\n' "$4" "$1" "$got"; _p=$((_p+1)) ;;
          *)     printf 'FAIL  %-46s %s -> %s, want %s*\n' "$4" "$1" "$got" "$3"; _f=$((_f+1)) ;;
        esac
    }
    tcase us          'apexzed1' yes  "us can type a Latin passphrase"
    tcase de          'apexzed1' yes  "de can type a Latin passphrase"
    tcase bg_bds-utf8 'apexzed1' yes  "bg_bds-utf8 can: Latin is its base plane"
    tcase ru          'apexzed1' yes  "ru can: Cyrillic is on AltGr, Latin is not"
    tcase jp106       'apexzed1' yes  "jp106 can, and only through its include"
    tcase hr-unicode  'apexzed1' no:  "hr-unicode cannot — no x anywhere"
    tcase vn          'apexzed1' no:  "vn cannot — no digit 1 anywhere"
    tcase fa          'apexzed1' no:  "fa cannot — almost no Latin at all"
    tcase hr-unicode  'alpha123' yes  "hr-unicode CAN type a passphrase avoiding q w x y"
    tcase nosuchkeymapatall 'apexzed1' unknown "a keymap that does not exist is unknown, not no"

    # THE FALLBACK'S OWN PRECONDITION. When a layout cannot type the
    # passphrase the engine moves the unlock prompt to `us`. That is only safe
    # if `us` can type everything the engine allows — which is every printable
    # ASCII character, the set the passphrase validation enforces.
    allascii=$(awk 'BEGIN { for (i = 32; i <= 126; i++) printf "%c", i }')
    got=$(keymap_can_type "$allascii" "$T" us)
    if [ "$got" = yes ]; then
        printf 'PASS  %-46s all 95 printable ASCII characters\n' "us can type anything the engine accepts"; _p=$((_p+1))
    else
        printf 'FAIL  %-46s us -> %s\n' "us can type anything the engine accepts" "$got"; _f=$((_f+1))
    fi

    # MUTATION 1: the X keysym name table. The legacy maps write their digits
    # as `one`, `two`; without the table they look like keyboards with no
    # number row, and bg_bds-utf8 flips to `no`. A table that cannot be made to
    # matter is not being tested.
    MUT="$WORK/mutant-keysym-names"
    sed '/^          nd = split("one two three/,/^          for (i = 1; i <= nd; i++)/d' "$TFNS" > "$MUT"
    if same_file "$TFNS" "$MUT"; then
        printf 'FAIL  %-46s the mutation matched nothing\n' "mutant: the keysym name table"; _f=$((_f+1))
    elif ! bash -n "$MUT" 2>/dev/null; then
        printf 'FAIL  %-46s the mutant does not parse\n' "mutant: the keysym name table"; _f=$((_f+1))
    else
        got=$(bash -c '. "$1"; keymap_can_type apexzed1 "$2" bg_bds-utf8' _ "$MUT" "$T" 2>/dev/null)
        case "$got" in
          no:*) printf 'PASS  %-46s names removed -> bg_bds-utf8 loses its digits (%s)\n' "mutant: the keysym name table" "$got"; _p=$((_p+1)) ;;
          *)    printf 'FAIL  %-46s got %s\n' "mutant: the keysym name table" "$got"; _f=$((_f+1)) ;;
        esac
    fi

    # MUTATION 2: the unanchored keycode pattern. Half the xkb maps write one
    # plane per line with the modifiers in front — `shift keycode 2 = ...`.
    # Anchoring the pattern made `fi` look like a keyboard with no `1`, no `7`
    # and no `e`, which is how this was found.
    MUT2="$WORK/mutant-anchor"
    sed 's|/keycode\[\[:space:\]\]+\[0-9\]+\[\[:space:\]\]\*=/ {|/^keycode[[:space:]]+[0-9]+[[:space:]]*=/ {|' "$TFNS" > "$MUT2"
    if same_file "$TFNS" "$MUT2"; then
        printf 'FAIL  %-46s the mutation matched nothing\n' "mutant: the unanchored keycode pattern"; _f=$((_f+1))
    else
        got=$(bash -c '. "$1"; keymap_can_type apexzed1 "$2" fi' _ "$MUT2" "$T" 2>/dev/null)
        case "$got" in
          no:*) printf 'PASS  %-46s anchored -> fi loses keys it has (%s)\n' "mutant: the unanchored keycode pattern" "$got"; _p=$((_p+1)) ;;
          *)    printf 'FAIL  %-46s got %s\n' "mutant: the unanchored keycode pattern" "$got"; _f=$((_f+1)) ;;
        esac
    fi

    # THE WHOLE TREE, so a future kbd update that moves a layout into or out of
    # the untypeable set is visible instead of silent. The bound is loose on
    # purpose: what must not happen is 0 (the check has stopped working) or
    # most of the tree (it has started condemning working keyboards).
    yes=0; no=0; unk=0
    for f in $(find "$T/xkb" "$T/legacy/i386" "$T/i386" -type f -name '*.map.gz' 2>/dev/null | sort); do
        # The FILE, not the name: keymap_ascii_set takes an absolute path as
        # the map itself, which skips one find over the whole tree per keymap.
        case "$(keymap_can_type apexzed1 "$T" "$f")" in
            yes)  yes=$((yes+1)) ;;
            no:*) no=$((no+1)) ;;
            *)    unk=$((unk+1)) ;;
        esac
    done
    if [ "$yes" -gt 400 ] && [ "$no" -ge 1 ] && [ "$no" -le 40 ]; then
        printf 'PASS  %-46s %s typeable, %s not, %s unreadable\n' "the tree splits the way it was measured" "$yes" "$no" "$unk"; _p=$((_p+1))
    else
        printf 'FAIL  %-46s %s typeable, %s not, %s unreadable\n' "the tree splits the way it was measured" "$yes" "$no" "$unk"; _f=$((_f+1))
    fi
    printf 'TYPEABLE-SUB: %s %s\n' "$_p" "$_f"
) > "$WORK/typeable.out" 2>&1
    cat "$WORK/typeable.out"
    tline=$(grep -m1 '^TYPEABLE-SUB: ' "$WORK/typeable.out" 2>/dev/null || true)
    if [ -z "$tline" ]; then
        bad "the typeability checks reported a result" "no TYPEABLE-SUB line"
    else
        read -r _ttag tp tf <<<"$tline"
        : "$_ttag"
        pass=$((pass + tp)); fail=$((fail + tf))
    fi
fi

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
