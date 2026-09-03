#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  Assertions for /usr/libexec/apex-labwc-keybinds — §17's fourth generator.
#
#  The shell has written Hyprland .conf/.lua and niri .kdl on every keybind edit
#  for a long time. labwc got nothing, because it has no IPC to push bindings
#  over and no include mechanism to append a generated file to. So on labwc the
#  Keybinds page was fully interactive and completely inert: rebind the
#  launcher, watch the UI confirm it, press the key, nothing happens.
#
#  Generation is a pure function of the model, so it is tested directly. The
#  splice is tested against fixture rc.xml files rather than a live one — this
#  suite must never touch ~/.config/labwc/rc.xml, and the helper's `apply`
#  default would do exactly that if it were called without --rc.
#
#      ./tests/test-labwc-keybinds.sh
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
# `set +e` is deliberate. This suite counts failures rather than aborting, and
# several assertions run commands that exit non-zero on purpose. CI invokes a
# script as `bash -e {0}`, under which a `x="$(cmd)"` assignment whose command
# fails kills the whole run part-way through.
set +e

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GEN="${ROOT}/files/system/libexec/apex-labwc-keybinds"
CHECK="${ROOT}/files/scripts/check-labwc-keybinds"
RC="${ROOT}/files/desktop/labwc/rc.xml"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

pass=0; fail=0
ok()  { printf 'PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL  %s\n' "$1"; fail=$((fail + 1)); }
skp() { printf 'SKIP  %s\n' "$1"; }
section() { printf '\n── %s ──\n' "$1"; }

[ -f "$GEN" ] || { printf 'missing %s\n' "$GEN" >&2; exit 1; }

# The model lives in the shell, so this suite needs a shell tree. The checkout
# first: the installed copy is whatever the last image shipped, which lags the
# tree under test, and checking a change against a stale source of truth fails
# for reasons unrelated to the change.
SHELL_TREE=""
for cand in "${ROOT}/../apex-shell" /usr/share/apex-shell; do
    [ -f "${cand}/src/services/config_tab/KeybindService.qml" ] && { SHELL_TREE="$cand"; break; }
done

if [ -z "$SHELL_TREE" ]; then
    printf 'no apex-shell tree available; nothing here can run\n' >&2
    printf '\npassed=0 failed=0 (skipped: no shell tree)\n'
    exit 0
fi

# What root._shellDir resolves to on a booted system. Fixed even though the
# model is read from a checkout — the seeded rc.xml is written for the installed
# location, not for whatever path this test happens to run from.
INSTALLED=/usr/share/apex-shell
g() { python3 "$GEN" "$@" --shell-dir "$SHELL_TREE" --shell-path "$INSTALLED"; }

section "the generator"

python3 -c "import ast; ast.parse(open('$GEN').read())" \
    && ok "apex-labwc-keybinds is valid Python" || bad "apex-labwc-keybinds is valid Python"

block="$(g print 2>/dev/null)"
[ -n "$block" ] && ok "it generates a non-empty block" || bad "it generates a non-empty block"

n="$(printf '%s\n' "$block" | grep -c '<keybind key=')"
[ "$n" -ge 30 ] \
    && ok "it generates every binding it can ($n)" \
    || bad "it generates every binding it can (got $n, expected >= 30)"

# A duplicate key is two bindings fighting over one shortcut, and labwc resolves
# that by taking one of them silently.
dupes="$(printf '%s\n' "$block" | grep -oE '<keybind key="[^"]+"' | sort | uniq -d)"
[ -z "$dupes" ] \
    && ok "no shortcut is generated twice" \
    || bad "duplicate shortcuts generated: $dupes"

printf '%s\n' "$block" | grep -q 'APEX-KEYBINDS-BEGIN' \
    && ok "the block carries its begin marker" || bad "the block carries its begin marker"
printf '%s\n' "$block" | grep -q 'APEX-KEYBINDS-END' \
    && ok "the block carries its end marker" || bad "the block carries its end marker"

# Deterministic: the splice compares against generated output, so an unstable
# ordering would make `check` fail at random.
b2="$(g print 2>/dev/null)"
[ "$block" = "$b2" ] && ok "generation is deterministic" || bad "generation is deterministic"

# The block has to be well-formed on its own, or the splice's post-validation is
# the only thing standing between a typo and an unparseable rc.xml.
printf '<r>%s</r>' "$block" > "$WORK/frag.xml"
python3 -c "import xml.etree.ElementTree as ET,sys; ET.parse(sys.argv[1])" "$WORK/frag.xml" \
    && ok "the generated block is well-formed XML" || bad "the generated block is well-formed XML"

section "what does not translate"

# labwc is a floating compositor: some tiling dispatchers have no equivalent.
# They must be REPORTED, not mapped onto something approximate — a shortcut
# doing the wrong thing is worse than one doing nothing.
skips="$(g print 2>/dev/null | grep -c 'no labwc equivalent')"
[ "$skips" -gt 0 ] \
    && ok "untranslatable bindings are reported ($skips)" \
    || bad "untranslatable bindings are reported"

g print 2>/dev/null | grep -q 'no labwc equivalent: window-pseudo' \
    && ok "pseudo-tiling is reported as unsupported" \
    || bad "pseudo-tiling is reported as unsupported"

# Nothing may be emitted with an unresolved Hyprland variable in it: labwc runs
# the command through execvp, so `$terminal` would be a literal argument.
printf '%s\n' "$block" | grep -q 'command="\$' \
    && bad "no unresolved config variable reaches a command" \
    || ok "no unresolved config variable reaches a command"

printf '%s\n' "$block" | grep -q 'command="alacritty"' \
    && ok "\$terminal is resolved to the installed terminal" \
    || bad "\$terminal is resolved to the installed terminal"

# The screenshot bindings embed a path. It has to be the INSTALLED one, not
# whatever tree the generator read the model out of.
printf '%s\n' "$block" | grep -q "command=\"bash ${INSTALLED}/src/scripts/screenshot.sh" \
    && ok "generated paths name the installed shell, not the build tree" \
    || bad "generated paths name the installed shell, not the build tree"

section "user overrides"

mkdir -p "$WORK/ov"
printf '{"dashboard-launcher": {"mods": "SUPER + SHIFT", "key": "P"}}' > "$WORK/ov/keybinds.json"
ovblock="$(g print --overrides "$WORK/ov/keybinds.json" 2>/dev/null)"

printf '%s\n' "$ovblock" | grep -A1 'key="W-S-p"' | grep -q 'apex shell launcher' \
    && ok "a rebind reaches the generated config" \
    || bad "a rebind reaches the generated config"

printf '%s\n' "$ovblock" | grep -q 'key="A-space"' \
    && bad "the replaced default is gone" \
    || ok "the replaced default is gone"

# A corrupt overrides file must fall back to the defaults, not produce an empty
# config — losing every shortcut is a much worse failure than ignoring an edit.
printf 'not json at all' > "$WORK/ov/bad.json"
badov="$(g print --overrides "$WORK/ov/bad.json" 2>/dev/null | grep -c '<keybind key=')"
[ "$badov" = "$n" ] \
    && ok "an unreadable overrides file falls back to the defaults" \
    || bad "an unreadable overrides file falls back to the defaults (got $badov, want $n)"

section "splicing into rc.xml"

# No markers yet: the block goes in before </keyboard>, where labwc expects
# keybinds, and not at the end of the file.
cat > "$WORK/fresh.xml" <<'XML'
<?xml version="1.0"?>
<!-- a header comment that must survive -->
<labwc_config>
  <keyboard>
    <keybind key="A-Tab"><action name="NextWindow"/></keybind>
  </keyboard>
</labwc_config>
XML
g apply --rc "$WORK/fresh.xml" --no-reload >/dev/null 2>&1
grep -q 'APEX-KEYBINDS-BEGIN' "$WORK/fresh.xml" \
    && ok "a file with no markers gets the block inserted" \
    || bad "a file with no markers gets the block inserted"
grep -q 'a header comment that must survive' "$WORK/fresh.xml" \
    && ok "the header comment survives the splice" \
    || bad "the header comment survives the splice"
grep -q 'A-Tab' "$WORK/fresh.xml" \
    && ok "bindings the generator does not own survive" \
    || bad "bindings the generator does not own survive"
python3 -c "import xml.etree.ElementTree as ET,sys; ET.parse(sys.argv[1])" "$WORK/fresh.xml" \
    && ok "the spliced file parses" || bad "the spliced file parses"

# Idempotent. A second apply must replace the region, not stack another copy.
before="$(grep -c '<keybind key=' "$WORK/fresh.xml")"
g apply --rc "$WORK/fresh.xml" --no-reload >/dev/null 2>&1
after="$(grep -c '<keybind key=' "$WORK/fresh.xml")"
[ "$before" = "$after" ] \
    && ok "applying twice replaces rather than duplicates" \
    || bad "applying twice replaces rather than duplicates ($before then $after)"

# An override applied over an existing region replaces it.
g apply --rc "$WORK/fresh.xml" --overrides "$WORK/ov/keybinds.json" --no-reload >/dev/null 2>&1
grep -q 'key="W-S-p"' "$WORK/fresh.xml" && ! grep -q 'key="A-space"' "$WORK/fresh.xml" \
    && ok "re-applying with an override replaces the old shortcut" \
    || bad "re-applying with an override replaces the old shortcut"

section "refusals"

# An unparseable rc.xml is never rewritten. labwc already falls back to defaults
# silently on a broken config; overwriting it would destroy the user's own
# bindings along with whatever the real problem was.
printf '<labwc_config><keyboard></labwc_config>' > "$WORK/broken.xml"
cp "$WORK/broken.xml" "$WORK/broken.orig"
g apply --rc "$WORK/broken.xml" --no-reload >/dev/null 2>&1
rc=$?
[ "$rc" -ne 0 ] \
    && ok "an unparseable rc.xml is refused" || bad "an unparseable rc.xml is refused"
cmp -s "$WORK/broken.xml" "$WORK/broken.orig" \
    && ok "the unparseable file is left untouched" \
    || bad "the unparseable file is left untouched"

# No </keyboard> means there is nowhere correct to put the block.
printf '<labwc_config><theme/></labwc_config>' > "$WORK/nokbd.xml"
cp "$WORK/nokbd.xml" "$WORK/nokbd.orig"
g apply --rc "$WORK/nokbd.xml" --no-reload >/dev/null 2>&1
[ $? -ne 0 ] && ok "a file with no <keyboard> is refused" || bad "a file with no <keyboard> is refused"
cmp -s "$WORK/nokbd.xml" "$WORK/nokbd.orig" \
    && ok "that file is left untouched too" || bad "that file is left untouched too"

# A missing file is a skip, not a crash: a user who has never launched labwc has
# no rc.xml, and saving a keybind must not fail because of it.
g apply --rc "$WORK/does-not-exist.xml" --no-reload >/dev/null 2>&1
[ $? -ne 0 ] && ok "a missing rc.xml reports rather than crashing" \
             || bad "a missing rc.xml reports rather than crashing"

section "the build-time check"

python3 "$CHECK" "$SHELL_TREE" "$RC" >/dev/null 2>&1 \
    && ok "the seeded rc.xml is exactly what the generator produces" \
    || { python3 "$CHECK" "$SHELL_TREE" "$RC" 2>&1 | head -20
         bad "the seeded rc.xml is exactly what the generator produces"; }

# The negative control. Without it, a check that silently passed on everything
# would look identical to a check that works.
cp "$RC" "$WORK/mutated.xml"
sed -i 's|<keybind key="A-space">|<keybind key="W-space">|' "$WORK/mutated.xml"
python3 "$CHECK" "$SHELL_TREE" "$WORK/mutated.xml" >/dev/null 2>&1 \
    && bad "the check fails on a drifted rc.xml" \
    || ok "the check fails on a drifted rc.xml"

# A file with no APEX region at all must fail rather than pass vacuously.
sed '/APEX-KEYBINDS-BEGIN/,/APEX-KEYBINDS-END/d' "$RC" > "$WORK/noregion.xml"
python3 "$CHECK" "$SHELL_TREE" "$WORK/noregion.xml" >/dev/null 2>&1 \
    && bad "the check fails when the region is missing entirely" \
    || ok "the check fails when the region is missing entirely"

section "the shell asks for it"

KS="${SHELL_TREE}/src/services/config_tab/KeybindService.qml"
if [ -f "$KS" ]; then
    grep -q 'apex-labwc-keybinds' "$KS" \
        && ok "KeybindService invokes the generator" \
        || bad "KeybindService invokes the generator"
    # The shell is a \$HOME checkout that updates independently of the image the
    # helper ships in, so a missing helper must not log a failed spawn on every
    # save.
    grep -q 'test -x /usr/libexec/apex-labwc-keybinds' "$KS" \
        && ok "it checks the helper exists before spawning it" \
        || bad "it checks the helper exists before spawning it"
else
    skp "no KeybindService.qml to check"
fi

printf '\npassed=%d failed=%d\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
