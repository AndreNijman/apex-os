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
# `set +e` is deliberate and load-bearing. This suite COUNTS failures rather
# than aborting on them, and several assertions run commands that exit non-zero
# on purpose — a refusal, a guard firing, a bad argument. GitHub Actions invokes
# a script as `bash -e {0}`, and under `-e` a `x="$(cmd)"` assignment whose
# command exits non-zero terminates the whole script. That is exactly what
# happened: the suite passed locally, and on CI it died part-way through with
# the remaining assertions reported as failures.
set +e

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GEN="${ROOT}/files/system/libexec/apex-input-apply"
TMPL="${ROOT}/files/desktop/labwc"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

pass=0; fail=0; skip=0
ok()  { printf 'PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL  %s\n' "$1"; fail=$((fail + 1)); }
# Counted, and reported in the summary. An uncounted skip is how a suite ends up
# reporting a clean pass for assertions that never ran — which is the same
# failure mode as the bug this file's niri section exists to close.
skp() { printf 'SKIP  %s\n' "$1"; skip=$((skip + 1)); }
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

# ─────────────────────────────────────────────────────────────────────────────
#  niri: the generated file has to be REACHABLE, not merely written.
#
#  Every assertion above this line proves apex-input-apply produced a file niri
#  would accept. None of them proved niri ever READS it — and it did not. The
#  generator emitted a comment telling the user to hand-add the `include` line
#  themselves, while Hyprland got a real `source =` in the shipped template. So
#  a niri user changed a touchpad setting, the Settings page reported success,
#  the .kdl was written, and nothing happened. Same for keybinds.
#
#  niri DOES have an include mechanism (top-level `include`, since 25.11; the
#  image ships 26.04), so nothing had to be invented — the line just was not
#  being written. apex-shell-firstrun writes it now, and this section drives
#  that block from the shipped provisioner rather than restating it.
#
#  The proof that matters is the LAST one: break the included file and niri must
#  reject the config that includes it. A config that still validates is a config
#  that never read it.
# ─────────────────────────────────────────────────────────────────────────────
FIRSTRUN="${ROOT}/files/system/libexec/apex-shell-firstrun"

section "niri: the include is wired by the provisioner, not by the user"

# Extracted by its own comment marker, so a renamed or deleted block fails here
# instead of silently skipping. `^    fi$` is the block's own closing fi — its
# inner ones are indented deeper.
INC_BLOCK="${WORK}/niri-include-block.sh"
sed -n '/^    # ── 6a\. the generated configs must actually be INCLUDED/,/^    fi$/p' \
    "$FIRSTRUN" > "$INC_BLOCK"
PRE_BLOCK="${WORK}/niri-precreate-block.sh"
sed -n '/^\[ -f "${CFG_DIR}\/ApexShellInput.kdl" \]/p' "$FIRSTRUN" > "$PRE_BLOCK"

if [ ! -s "$INC_BLOCK" ]; then
    bad "the provisioner's niri include block is where this suite drives it"
else
    ok "the provisioner's niri include block is where this suite drives it"
    bash -n "$INC_BLOCK" \
        && ok "the extracted block is self-contained bash" \
        || bad "the extracted block is self-contained bash"
fi

[ -s "$PRE_BLOCK" ] \
    && ok "ApexShellInput.kdl has a pre-create in the provisioner" \
    || bad "ApexShellInput.kdl has a pre-create in the provisioner"

# ── path agreement, derived from source on both sides ────────────────────────
# Hardcoding the two paths here would pass forever after a rename on either
# side. The Hyprland equivalent of this check in test-apex-firstrun.sh carries
# the note that forgetting it "has already happened once per file added".
inc_paths="$(sed -n 's|^include "\${CFG_DIR}/\(.*\)"$|\1|p' "$INC_BLOCK")"
[ "$(printf '%s\n' "$inc_paths" | grep -c .)" = 2 ] \
    && ok "the block includes exactly two generated files" \
    || bad "the block includes exactly two generated files (got: ${inc_paths})"

# apex-input-apply's own NIRI_OUT, read out of the shipped script.
gen_out="$(python3 - "$GEN" <<'PY'
import re, sys
src = open(sys.argv[1]).read()
m = re.search(r'^NIRI_OUT = os\.path\.join\(HOME, "(.*)"\)$', src, re.M)
print(m.group(1) if m else "")
PY
)"
[ -n "$gen_out" ] \
    && ok "apex-input-apply declares NIRI_OUT" || bad "apex-input-apply declares NIRI_OUT"
printf '%s\n' "$inc_paths" | grep -qxF "${gen_out#.config/apex-shell/}" \
    && ok "the include path is exactly the file apex-input-apply writes" \
    || bad "the include path is exactly the file apex-input-apply writes (${gen_out})"

# The keybind half is generated by APEX Shell, so the agreement is with the
# shell tree when one is available. Same lookup order as test-apex-firstrun.sh.
SHELL_TREE=""
for cand in "${ROOT}/../apex-shell" /usr/share/apex-shell; do
    [ -f "${cand}/src/services/config_tab/KeybindService.qml" ] && { SHELL_TREE="$cand"; break; }
done
if [ -z "$SHELL_TREE" ]; then
    skp "no apex-shell tree: cannot cross-check the keybind include path"
else
    kdl_name="$(sed -n 's|.*_kdlPath: *_configDir + "/\([^"]*\)".*|\1|p' \
        "${SHELL_TREE}/src/services/config_tab/KeybindService.qml" | head -1)"
    if [ -z "$kdl_name" ]; then
        bad "KeybindService declares _kdlPath"
    else
        printf '%s\n' "$inc_paths" | grep -qxF "$kdl_name" \
            && ok "the include path is exactly the file the shell writes (${kdl_name})" \
            || bad "the include path is exactly the file the shell writes (${kdl_name})"
    fi
fi

# ── driving the real block ───────────────────────────────────────────────────
# `niri validate` is a decision point inside the block (refuse a config that was
# already broken; restore the backup if the includes break one that was not), so
# the append logic is exercised against a STUB that answers on demand. That is
# the seam, not a re-implementation: the stub decides nothing about KDL, it only
# returns the verdict the case under test needs. Real niri runs further down.
STUBBIN="${WORK}/stubbin"
mkdir -p "$STUBBIN"
cat > "${STUBBIN}/niri" <<'STUB'
#!/usr/bin/env bash
exit "${STUB_NIRI_EXIT:-0}"
STUB
chmod 0755 "${STUBBIN}/niri"

# Refuse to run the stubbed cases if the stub is not what resolves — otherwise
# a machine with real niri would quietly test something else.
if [ "$(PATH="${STUBBIN}" command -v niri)" != "${STUBBIN}/niri" ]; then
    bad "the niri stub is what resolves for the stubbed cases"
else
    ok "the niri stub is what resolves for the stubbed cases"
fi

# A throwaway HOME with a seeded niri config and both include targets present,
# exactly as the provisioner leaves them by the time block 6a runs.
mkniri() {
    local h="$1"
    mkdir -p "$h/.config/niri" "$h/.config/apex-shell"
    if [ -f /usr/share/doc/niri/default-config.kdl ]; then
        cp /usr/share/doc/niri/default-config.kdl "$h/.config/niri/config.kdl"
    else
        printf 'input {\n    touchpad {\n        tap\n    }\n}\n' > "$h/.config/niri/config.kdl"
    fi
    printf '\n// APEX Shell autostarts (seeded by apex-shell-firstrun)\nspawn-at-startup "/usr/libexec/apex-shell-autostart"\n' \
        >> "$h/.config/niri/config.kdl"
    : > "$h/.config/apex-shell/ApexShellInput.kdl"
    : > "$h/.config/apex-shell/ApexShellKeybinds.kdl"
}

# The block reads HOME, CFG_DIR, NIRI_CONF and log(); the harness supplies those
# and nothing else, so nothing here can drift from the shipped script.
run_inc() {
    local h="$1" p="${2:-$PATH}"
    HOME="$h" CFG_DIR="$h/.config/apex-shell" NIRI_CONF="$h/.config/niri/config.kdl" \
    PATH="$p" bash -c 'set -euo pipefail; log() { printf "%s\n" "$*"; }; source "$1"' \
        -- "$INC_BLOCK" 2>&1
}

section "niri: the include line is appended, and nothing else changes"
h="${WORK}/niri-ok"; mkniri "$h"
NC="$h/.config/niri/config.kdl"
before_lines="$(wc -l < "$NC")"
before_sum="$(sha256sum < "$NC" | cut -d' ' -f1)"
before_bytes="$(wc -c < "$NC")"
out="$(run_inc "$h" "${STUBBIN}:${PATH}")"
printf '%s\n' "$out" | sed 's/^/      /'

grep -qF "include \"$h/.config/apex-shell/ApexShellInput.kdl\"" "$NC" \
    && ok "the generated input config is included" || bad "the generated input config is included"
grep -qF "include \"$h/.config/apex-shell/ApexShellKeybinds.kdl\"" "$NC" \
    && ok "the generated keybind config is included" || bad "the generated keybind config is included"

# Grepping for what was added cannot detect what was deleted, so the whole
# former content must still be there byte-for-byte, at the front.
[ "$(head -c "$before_bytes" "$NC" | sha256sum | cut -d' ' -f1)" = "$before_sum" ] \
    && ok "every byte the user already had is untouched" \
    || bad "every byte the user already had is untouched"
[ "$(wc -l < "$NC")" -eq "$((before_lines + 6))" ] \
    && ok "exactly six lines were added" \
    || bad "exactly six lines were added ($before_lines -> $(wc -l < "$NC"))"
grep -qF 'spawn-at-startup "/usr/libexec/apex-shell-autostart"' "$NC" \
    && ok "the autostart landmark survives" || bad "the autostart landmark survives"
[ -f "${NC}.pre-include.bak" ] \
    && ok "a backup was taken before the edit" || bad "a backup was taken before the edit"

run_inc "$h" "${STUBBIN}:${PATH}" >/dev/null 2>&1
[ "$(grep -c '^include ' "$NC")" = 2 ] \
    && ok "re-running adds nothing (its own marker, not the autostart block's)" \
    || bad "re-running adds nothing (its own marker, not the autostart block's)"

section "niri: an include is never written for a file that is not there"
# A missing include target is a HARD parse error in niri and takes the whole
# config with it, so this refusal is the difference between a partial feature
# and a session with no keybinds at all.
h="${WORK}/niri-missing"; mkniri "$h"
rm -f "$h/.config/apex-shell/ApexShellInput.kdl"
NC="$h/.config/niri/config.kdl"
sum="$(sha256sum < "$NC" | cut -d' ' -f1)"
out="$(run_inc "$h" "${STUBBIN}:${PATH}")"
printf '%s\n' "$out" | grep -q 'include targets are missing' \
    && ok "it refuses and says which way it refused" || bad "it refuses and says which way it refused"
[ "$(sha256sum < "$NC" | cut -d' ' -f1)" = "$sum" ] \
    && ok "the config is left exactly as it was" || bad "the config is left exactly as it was"

section "niri: a config that was already broken is not touched"
h="${WORK}/niri-broken"; mkniri "$h"
NC="$h/.config/niri/config.kdl"
sum="$(sha256sum < "$NC" | cut -d' ' -f1)"
out="$(STUB_NIRI_EXIT=1 run_inc "$h" "${STUBBIN}:${PATH}")"
printf '%s\n' "$out" | grep -q 'refusing to touch it' \
    && ok "it refuses and says so" || bad "it refuses and says so"
[ "$(sha256sum < "$NC" | cut -d' ' -f1)" = "$sum" ] \
    && ok "a pre-broken config is left exactly as it was" \
    || bad "a pre-broken config is left exactly as it was"

section "niri: if the includes break the config, the backup goes back"
# Deterministic only with a stub: valid on the first call, rejected on the
# second. Real niri will never produce this, which is exactly why the restore
# path would otherwise never be executed by anything.
h="${WORK}/niri-restore"; mkniri "$h"
NC="$h/.config/niri/config.kdl"
sum="$(sha256sum < "$NC" | cut -d' ' -f1)"
cat > "${STUBBIN}/niri" <<'STUB'
#!/usr/bin/env bash
c="${STUB_NIRI_COUNT:-/dev/null}"
n=0; [ -f "$c" ] && n="$(cat "$c")"
n=$((n + 1)); [ "$c" = /dev/null ] || printf '%s' "$n" > "$c"
[ "$n" -ge 2 ] && exit 1
exit 0
STUB
chmod 0755 "${STUBBIN}/niri"
out="$(STUB_NIRI_COUNT="${WORK}/niri-count" run_inc "$h" "${STUBBIN}:${PATH}")"
printf '%s\n' "$out" | grep -q 'restored the backup' \
    && ok "it restores and says so" || bad "it restores and says so"
[ "$(sha256sum < "$NC" | cut -d' ' -f1)" = "$sum" ] \
    && ok "the restored config is byte-identical to the original" \
    || bad "the restored config is byte-identical to the original"

section "niri: the compositor really reads what the generator wrote"
# The only assertion that can tell a live include from a decorative one. It
# needs the real binary; there is none on the CI runner, so this skips there and
# runs on any APEX machine and in the image build (Containerfile.base).
if ! command -v niri >/dev/null 2>&1; then
    skp "niri unavailable: the include-reachability proof did not run"
    skp "niri unavailable: a broken include is not proven to be rejected"
    skp "niri unavailable: the generated input values are not proven reachable"
else
    h="${WORK}/niri-live"; mkniri "$h"
    NC="$h/.config/niri/config.kdl"
    run_inc "$h" >/dev/null 2>&1
    niri validate --config "$NC" >/dev/null 2>&1 \
        && ok "niri accepts config.kdl with both includes" \
        || bad "niri accepts config.kdl with both includes"

    # THE assertion. If niri still validates a config whose included file is
    # garbage, the include is not being read and every check above is theatre.
    printf 'this-is-not-a-niri-node\n' > "$h/.config/apex-shell/ApexShellInput.kdl"
    niri validate --config "$NC" >/dev/null 2>&1 \
        && bad "a broken include is rejected — proving config.kdl really reads it" \
        || ok "a broken include is rejected — proving config.kdl really reads it"

    # And the real generator's output, reached the same way: settings written by
    # apex-input-apply arrive through the include the provisioner added.
    mkdir -p "$h/.config/labwc" "$h/.config/hypr"
    cp "${TMPL}/rc.xml" "${TMPL}/menu.xml" "$h/.config/labwc/"
    sed 's/@ACCENT@/#D9F99D/g' "${TMPL}/themerc-override" > "$h/.config/labwc/themerc-override"
    printf '{"keyboard":{"repeat_rate":42}}\n' > "$h/.config/apex-shell/input.json"
    run_gen "$h" >/dev/null 2>&1
    if niri validate --config "$NC" >/dev/null 2>&1 \
       && grep -q 'repeat-rate 42' "$h/.config/apex-shell/ApexShellInput.kdl"; then
        ok "a setting changed in Settings reaches niri through the include"
    else
        bad "a setting changed in Settings reaches niri through the include"
    fi
fi

printf '\napex-input: %d passed, %d failed, %d skipped\n' "$pass" "$fail" "$skip"
[ "$fail" -eq 0 ]
