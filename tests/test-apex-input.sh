#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  Assertions for /usr/libexec/apex-input-apply — the compositor-neutral input
#  settings generator.
#
#  Three properties matter more than the individual mappings:
#
#    1. The DEFAULTS are a no-op against the configs the image already ships.
#       A settings page whose defaults silently change behaviour on first use is
#       worse than no settings page.
#    2. The in-place rc.xml edit is LOSSLESS outside <libinput>. labwc has no
#       include mechanism, so this is the only file it can be written to, and it
#       carries a forty-line header plus a comment on nearly every decision. An
#       earlier ElementTree round-trip deleted that header silently.
#    3. Every generated config is ACCEPTED by the compositor that consumes it.
#       Names are verified, not remembered: labwc ignores an unknown element in
#       silence, which is how `<tapToClick>` shipped doing nothing.
#
#  Skips cleanly where a compositor is not installed.
#      ./tests/test-apex-input.sh
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GEN="${ROOT}/files/system/libexec/apex-input-apply"
TMPL="${ROOT}/files/desktop/labwc"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

pass=0; fail=0
ok()  { printf 'PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL  %s\n' "$1"; fail=$((fail + 1)); }
skp() { printf 'SKIP  %s\n' "$1"; }
section() { printf '\n── %s ──\n' "$1"; }

[ -f "$GEN" ] || { printf 'missing %s\n' "$GEN" >&2; exit 1; }

# A throwaway HOME seeded with the SHIPPED configs, so every assertion runs
# against what users actually get.
mkhome() {
    local h="$1"
    mkdir -p "$h/.config/labwc" "$h/.config/apex-shell" "$h/.config/hypr"
    cp "${TMPL}/rc.xml" "${TMPL}/menu.xml" "$h/.config/labwc/"
    sed 's/@ACCENT@/#D9F99D/g' "${TMPL}/themerc-override" > "$h/.config/labwc/themerc-override"
}

run_gen() { HOME="$1" python3 "$GEN" --no-reload "${@:2}"; }

section "the generator runs and self-tests"
python3 -c "import ast,sys; ast.parse(open('$GEN').read())" \
    && ok "apex-input-apply is valid Python" || bad "apex-input-apply is valid Python"

# --self-test generates from the defaults and runs each compositor's own
# validator. It reports SKIP for a compositor that is absent, so the only
# failure mode here is a mapping the compositor rejects.
st="$(python3 "$GEN" --self-test 2>&1)"
printf '%s\n' "$st" | sed 's/^/      /'
printf '%s\n' "$st" | grep -q '^FAIL' \
    && bad "every installed compositor accepts the generated config" \
    || ok "every installed compositor accepts the generated config"

section "defaults are a no-op against the shipped config"
h="${WORK}/defaults"; mkhome "$h"
cp "$h/.config/labwc/rc.xml" "${WORK}/rc.orig"
echo '{}' > "$h/.config/apex-shell/input.json"
run_gen "$h" >/dev/null 2>&1

# The shipped rc.xml sets naturalScroll/tap/tapAndDrag/accelProfile on the
# touchpad. The defaults must reproduce all four, or a user opening Settings for
# the first time silently changes their own touchpad.
for want in '<naturalScroll>yes</naturalScroll>' '<tap>yes</tap>' \
            '<tapAndDrag>yes</tapAndDrag>' '<accelProfile>adaptive</accelProfile>'; do
    sed -n '/<device category="touchpad"/,/<\/device>/p' "$h/.config/labwc/rc.xml" \
        | grep -qF "$want" \
        && ok "default reproduces ${want}" || bad "default reproduces ${want}"
done

section "the rc.xml edit is lossless outside <libinput>"
diff <(sed '/<libinput>/,/<\/libinput>/d' "${WORK}/rc.orig") \
     <(sed '/<libinput>/,/<\/libinput>/d' "$h/.config/labwc/rc.xml") >/dev/null \
    && ok "everything outside <libinput> is byte-identical" \
    || bad "everything outside <libinput> is byte-identical"

# The header comment sits OUTSIDE the root element, where an ElementTree
# round-trip cannot represent it. This is the assertion that caught that.
head -8 "$h/.config/labwc/rc.xml" | grep -q 'APEX-OS' \
    && ok "the file header comment survives" || bad "the file header comment survives"

before="$(grep -c '<!--' "${WORK}/rc.orig")"
after="$(grep -c '<!--' "$h/.config/labwc/rc.xml")"
[ "$before" = "$after" ] \
    && ok "all $before comments survive" || bad "all comments survive (${before} -> ${after})"

if command -v xmllint >/dev/null 2>&1; then
    xmllint --noout "$h/.config/labwc/rc.xml" 2>/dev/null \
        && ok "the rewritten rc.xml is well-formed" || bad "the rewritten rc.xml is well-formed"
else
    skp "xmllint unavailable"
fi

section "settings reach every compositor"
h="${WORK}/applied"; mkhome "$h"
cat > "$h/.config/apex-shell/input.json" <<'JSON'
{ "touchpad": { "tap": false, "natural_scroll": false, "click_method": "buttonAreas",
                "speed": 0.5, "three_finger_drag": true, "tap_button_map": "lmr" },
  "pointer":  { "left_handed": true, "speed": -0.2 },
  "keyboard": { "repeat_rate": 40, "repeat_delay": 300 } }
JSON
run_gen "$h" >/dev/null 2>&1

tp() { sed -n '/<device category="touchpad"/,/<\/device>/p' "$h/.config/labwc/rc.xml"; }
[ "$(tp | sed -n 's|.*<tap>\(.*\)</tap>.*|\1|p')" = "no" ] \
    && ok "labwc: tap disabled" || bad "labwc: tap disabled"
[ "$(tp | sed -n 's|.*<clickMethod>\(.*\)</clickMethod>.*|\1|p')" = "buttonAreas" ] \
    && ok "labwc: clickMethod applied" || bad "labwc: clickMethod applied"
[ "$(tp | sed -n 's|.*<tapButtonMap>\(.*\)</tapButtonMap>.*|\1|p')" = "lmr" ] \
    && ok "labwc: tapButtonMap applied" || bad "labwc: tapButtonMap applied"
tp | grep -q threeFingerDrag \
    && ok "labwc: threeFingerDrag emitted only when enabled" \
    || bad "labwc: threeFingerDrag emitted only when enabled"
grep -q '<leftHanded>yes</leftHanded>' "$h/.config/labwc/rc.xml" \
    && ok "labwc: leftHanded reaches the pointer device" || bad "labwc: leftHanded reaches the pointer device"

H="$h/.config/hypr/apex-input.conf"
grep -q 'tap-to-click = false' "$H" && ok "hyprland: tap-to-click applied" || bad "hyprland: tap-to-click applied"
grep -q 'left_handed = true'   "$H" && ok "hyprland: left_handed applied"   || bad "hyprland: left_handed applied"
grep -q 'repeat_rate = 40'     "$H" && ok "hyprland: repeat_rate applied"   || bad "hyprland: repeat_rate applied"
grep -q 'tap_button_map = lmr' "$H" && ok "hyprland: tap_button_map applied" || bad "hyprland: tap_button_map applied"

N="$h/.config/apex-shell/ApexShellInput.kdl"
# niri expresses a false boolean by OMITTING the flag, so `tap` must be absent.
grep -qE '^\s+tap$' "$N" \
    && bad "niri: a disabled boolean is omitted, not written false" \
    || ok "niri: a disabled boolean is omitted, not written false"
grep -q 'click-method "button-areas"' "$N" && ok "niri: click-method translated" || bad "niri: click-method translated"
grep -q 'tap-button-map "left-middle-right"' "$N" && ok "niri: tap-button-map translated" || bad "niri: tap-button-map translated"
grep -q 'repeat-rate 40' "$N" && ok "niri: repeat-rate applied" || bad "niri: repeat-rate applied"

if command -v niri >/dev/null 2>&1; then
    niri validate --config "$N" >/dev/null 2>&1 \
        && ok "niri validates the generated file with non-default values" \
        || bad "niri validates the generated file with non-default values"
else
    skp "niri unavailable"
fi

section "bad input is corrected, not obeyed"
h="${WORK}/bad"; mkhome "$h"
cat > "$h/.config/apex-shell/input.json" <<'JSON'
{ "touchpad": { "speed": 99, "accel_profile": "turbo", "click_method": "wishful",
                "tap_button_map": "xyz", "scroll_factor": 0 },
  "keyboard": { "repeat_rate": 9999 },
  "nonsense": { "whatever": 1 } }
JSON
notes="$(run_gen "$h" 2>&1)"
printf '%s\n' "$notes" | grep -q 'must be between -1.0 and 1.0' \
    && ok "an out-of-range speed is clamped and reported" || bad "an out-of-range speed is clamped and reported"
printf '%s\n' "$notes" | grep -q "accel_profile" \
    && ok "an unknown accel profile is corrected and reported" || bad "an unknown accel profile is corrected and reported"
grep -q '<accelProfile>adaptive</accelProfile>' "$h/.config/labwc/rc.xml" \
    && ok "the corrected value is what gets written" || bad "the corrected value is what gets written"
if command -v xmllint >/dev/null 2>&1; then
    xmllint --noout "$h/.config/labwc/rc.xml" 2>/dev/null \
        && ok "nonsense input still yields valid XML" || bad "nonsense input still yields valid XML"
fi

section "an unparseable rc.xml is never overwritten"
h="${WORK}/broken"; mkhome "$h"
printf '<?xml version="1.0"?>\n<labwc_config><core>\n' > "$h/.config/labwc/rc.xml"
cp "$h/.config/labwc/rc.xml" "${WORK}/broken.orig"
echo '{}' > "$h/.config/apex-shell/input.json"
run_gen "$h" 2>&1 | grep -q 'refusing to touch it' \
    && ok "it refuses and says so" || bad "it refuses and says so"
cmp -s "$h/.config/labwc/rc.xml" "${WORK}/broken.orig" \
    && ok "the broken file is left exactly as it was" || bad "the broken file is left exactly as it was"

section "idempotence"
h="${WORK}/idem"; mkhome "$h"
echo '{"touchpad":{"tap":false}}' > "$h/.config/apex-shell/input.json"
run_gen "$h" >/dev/null 2>&1
cp "$h/.config/labwc/rc.xml" "${WORK}/idem.1"
run_gen "$h" >/dev/null 2>&1
cmp -s "$h/.config/labwc/rc.xml" "${WORK}/idem.1" \
    && ok "re-running changes nothing" || bad "re-running changes nothing"

printf '\napex-input: %d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
