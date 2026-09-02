#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  Assertions for /usr/libexec/apex-display-apply.
#
#  The generators are pure functions of the model, so they are tested directly
#  and exhaustively. The parts that need real outputs — enumeration and a live
#  apply — are exercised only when this runs inside a session that has them, and
#  skipped otherwise rather than faked.
#
#      ./tests/test-apex-display.sh
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
GEN="${ROOT}/files/system/libexec/apex-display-apply"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

pass=0; fail=0
ok()  { printf 'PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL  %s\n' "$1"; fail=$((fail + 1)); }
skp() { printf 'SKIP  %s\n' "$1"; }
section() { printf '\n── %s ──\n' "$1"; }

[ -f "$GEN" ] || { printf 'missing %s\n' "$GEN" >&2; exit 1; }

section "the generators"
python3 -c "import ast; ast.parse(open('$GEN').read())" \
    && ok "apex-display-apply is valid Python" || bad "apex-display-apply is valid Python"

st="$(python3 "$GEN" --self-test 2>&1)"
printf '%s\n' "$st" | sed 's/^/      /'
printf '%s\n' "$st" | grep -q '^FAIL' \
    && bad "the generator self-test passes" || ok "the generator self-test passes"

# Fake compositor tools, each recording its invocation. Two jobs:
#   * determinism — kanshi profiles are keyed on the CONNECTED outputs, so a
#     real enumeration would make these assertions depend on how many monitors
#     the host has plugged in;
#   * proof of isolation — see the section further down.
# They print nothing, so enumeration yields no outputs and the generator falls
# back to the model, which is what makes the expected output fixed.
#
# `pkill` is faked too, and that one is not about determinism. `save` ends with
#     pkill -HUP -x kanshi
# so that a written profile takes effect, and pkill does not care what HOME is:
# an unfaked run signals the REAL kanshi on the developer's live session. Same
# class of bug as the one this file exists because of, one process along.
FAKE="${WORK}/fakebin"; mkdir -p "$FAKE"
for tool in hyprctl wlr-randr pkill; do
    printf '#!/bin/sh\necho "$0 $*" >> "%s/called"\nexit 0\n' "$WORK" > "${FAKE}/${tool}"
    chmod +x "${FAKE}/${tool}"
done

# PATH is REPLACED, not prefixed. Prefixing leaves the real tools reachable, so
# a typo in a fake name silently falls through to the live compositor — the
# failure mode has to be "no tool at all", never "the real one". That means
# python3 has to be named absolutely, since it can no longer be found on PATH.
PY="$(command -v python3)"
[ -x "$PY" ] || { printf 'no python3\n' >&2; exit 1; }
for tool in hyprctl wlr-randr pkill; do
    [ -x "${FAKE}/${tool}" ] || { printf 'fake %s missing\n' "$tool" >&2; exit 1; }
done

# Every invocation below goes through one of these two. Nothing in this file
# calls the generator with the real PATH, so no assertion can reach a real
# compositor even if a future edit forgets which action it is using.
run_save() { PATH="$FAKE" HOME="$1" APEX_DISPLAY_NO_LIVE=1 "$PY" "$GEN" save; }
run_gen()  { local h="$1"; shift; PATH="$FAKE" HOME="$h" APEX_DISPLAY_NO_LIVE=1 "$PY" "$GEN" "$@"; }

section "the model is validated, not trusted"
H="${WORK}/home"; mkdir -p "$H/.config/apex-shell" "$H/.config/hypr" "$H/.config/kanshi"
cat > "$H/.config/apex-shell/display.json" <<'JSON'
{ "outputs": [
  { "name": "eDP-1", "enabled": true, "scale": 99, "transform": "sideways" },
  { "enabled": true, "scale": 1 } ] }
JSON
# The validation notes are printed before the action branch is reached, so they
# are readable even though the guard then refuses this `apply`.
notes="$(run_gen "$H" apply --dry-run 2>&1)"
printf '%s\n' "$notes" | grep -q 'scale 99' \
    && ok "an out-of-range scale is corrected and reported" || bad "an out-of-range scale is corrected and reported"
printf '%s\n' "$notes" | grep -q "unknown transform" \
    && ok "an unknown transform is corrected and reported" || bad "an unknown transform is corrected and reported"
printf '%s\n' "$notes" | grep -q 'no name' \
    && ok "an entry with no output name is skipped and reported" || bad "an entry with no output name is skipped and reported"

section "persistence is written for both backends"
H2="${WORK}/home2"; mkdir -p "$H2/.config/apex-shell" "$H2/.config/hypr" "$H2/.config/kanshi"
cat > "$H2/.config/apex-shell/display.json" <<'JSON'
{ "outputs": [
  { "name": "eDP-1", "enabled": true, "x": 0, "y": 0, "scale": 1.5,
    "transform": "normal", "adaptive_sync": false,
    "mode": { "width": 2560, "height": 1600, "refresh": 165 } },
  { "name": "HDMI-A-1", "enabled": false } ] }
JSON
# `save`, NEVER `apply`. This is the whole reason the two actions are separate:
# `apply` reaches the RUNNING compositor through hyprctl/wlr-randr, and neither
# cares what HOME is. An earlier version of this file called `apply` here and
# pushed the fixture below — 2560x1600@165 at scale 1.5 — onto the live desktop
# it was running on, whose panel is 1920x1200. Isolating HOME is not isolation.
run_save "$H2" >/dev/null 2>&1

K="$H2/.config/kanshi/config"
[ -s "$K" ] && ok "a kanshi config is written" || bad "a kanshi config is written"
if [ -s "$K" ]; then
    grep -q '^profile apex-' "$K" && ok "kanshi: the profile is named for its output set" \
        || bad "kanshi: the profile is named for its output set"
    grep -q 'mode 2560x1600@165' "$K" && ok "kanshi: mode written without the Hz suffix" \
        || bad "kanshi: mode written without the Hz suffix"
    grep -q 'output "HDMI-A-1" disable' "$K" && ok "kanshi: a disabled output is disabled" \
        || bad "kanshi: a disabled output is disabled"
    grep -q 'scale 1.5' "$K" && ok "kanshi: fractional scale written" || bad "kanshi: fractional scale written"
fi

D="$H2/.config/hypr/apex-display.conf"
[ -s "$D" ] && ok "a Hyprland monitor file is written" || bad "a Hyprland monitor file is written"
if [ -s "$D" ]; then
    grep -q '^monitor=eDP-1,2560x1600@165,0x0,1.5$' "$D" \
        && ok "hyprland: the monitor line matches Hyprland's own syntax" \
        || bad "hyprland: the monitor line matches Hyprland's own syntax"
    grep -q '^monitor=HDMI-A-1,disable$' "$D" \
        && ok "hyprland: a disabled output is disabled" || bad "hyprland: a disabled output is disabled"
fi

# A written kanshi profile does nothing until kanshi re-reads it, so `save` has
# to signal it. Asserted because the failure is silent: the file is correct, the
# layout just never changes.
grep -q 'pkill .*-HUP.*kanshi' "${WORK}/called" 2>/dev/null \
    && ok "save signals kanshi to re-read the profile" \
    || bad "save signals kanshi to re-read the profile"

section "idempotence"
cp "$K" "${WORK}/k.1"; cp "$D" "${WORK}/d.1"
run_save "$H2" >/dev/null 2>&1
cmp -s "$K" "${WORK}/k.1" && cmp -s "$D" "${WORK}/d.1" \
    && ok "re-applying changes nothing" || bad "re-applying changes nothing"

section "a missing helper is a failure, not a traceback"
# The generator falls back to `pgrep` to identify the compositor when the
# environment gives no hint. `run()` did not tolerate a missing binary, so on a
# machine without pgrep on PATH `save` died with a FileNotFoundError instead of
# writing the layout — which is what happens on a CI runner and in any minimal
# container. PATH here holds ONLY the fakes, so pgrep is genuinely absent.
H6="${WORK}/home6"; mkdir -p "$H6/.config/apex-shell" "$H6/.config/hypr" "$H6/.config/kanshi"
cp "$H2/.config/apex-shell/display.json" "$H6/.config/apex-shell/display.json"
out="$(env -u HYPRLAND_INSTANCE_SIGNATURE -u NIRI_SOCKET -u XDG_CURRENT_DESKTOP \
        -u WAYLAND_DISPLAY PATH="$FAKE" HOME="$H6" APEX_DISPLAY_NO_LIVE=1 \
        "$PY" "$GEN" save 2>&1)"
printf '%s' "$out" | grep -qE "Traceback|FileNotFoundError" \
    && { bad "no traceback when a helper is missing"; printf '      %s\n' "$out" | head -3; } \
    || ok "no traceback when a helper is missing"
[ -s "$H6/.config/kanshi/config" ] \
    && ok "the layout is still written with no compositor and no pgrep" \
    || bad "the layout is still written with no compositor and no pgrep"

section "an empty model does nothing"
H3="${WORK}/home3"; mkdir -p "$H3/.config/apex-shell"
echo '{"outputs":[]}' > "$H3/.config/apex-shell/display.json"
run_gen "$H3" save 2>&1 | grep -q 'nothing to do' \
    && ok "an empty model is a no-op and says so" || bad "an empty model is a no-op and says so"
[ ! -e "$H3/.config/kanshi/config" ] \
    && ok "an empty model writes no persistence" || bad "an empty model writes no persistence"

section "a corrupt model is refused, not guessed at"
H4="${WORK}/home4"; mkdir -p "$H4/.config/apex-shell"
printf '{ not json' > "$H4/.config/apex-shell/display.json"
run_gen "$H4" save 2>&1 | grep -q 'not usable' \
    && ok "a corrupt model is reported" || bad "a corrupt model is reported"

section "a test can never reach the live compositor"
# Not a comment but a proof: the fake hyprctl/wlr-randr/pkill are the ONLY
# things on PATH, each recording that it was called. If `save` invokes a
# mutating one, the marker exists and this fails.
# The property is that no MUTATING call is made. `save` does enumerate — kanshi
# profiles are keyed on the connected outputs — and `hyprctl -j monitors` /
# `wlr-randr --json` are read-only, so those are expected and fine. What must
# never appear is a call that CHANGES anything: `hyprctl keyword` or a
# `wlr-randr --output`. That distinction is the whole bug: a test isolated HOME,
# assumed that was enough, and reconfigured the live desktop through hyprctl.
mutating() { grep -qE 'hyprctl .*keyword|wlr-randr .*--output' "${WORK}/called" 2>/dev/null; }

rm -f "${WORK}/called"
H5="${WORK}/home5"; mkdir -p "$H5/.config/apex-shell"
cp "$H2/.config/apex-shell/display.json" "$H5/.config/apex-shell/display.json"
run_save "$H5" >/dev/null 2>&1
if mutating; then
    printf '  it called: %s\n' "$(tr '\n' ' ' < "${WORK}/called")"
    bad "save makes no mutating compositor call"
else
    ok "save makes no mutating compositor call"
fi

rm -f "${WORK}/called"
out="$(PATH="$FAKE" HOME="$H5" APEX_DISPLAY_NO_LIVE=1 "$PY" "$GEN" apply 2>&1)"
printf '%s' "$out" | grep -q 'refusing to touch the live compositor' \
    && ok "apply refuses when APEX_DISPLAY_NO_LIVE is set" \
    || bad "apply refuses when APEX_DISPLAY_NO_LIVE is set"
mutating \
    && bad "the refusal prevents every mutating call" \
    || ok "the refusal prevents every mutating call"

# And without the guard it WOULD mutate — otherwise the guard proves nothing.
#
# `env -u` rather than just leaving the variable out: this is the one call in
# the file that deliberately disables the safety, so it must not depend on the
# ambient environment being clean. CI exports APEX_DISPLAY_NO_LIVE for the whole
# step, which would otherwise turn this assertion into a false failure — and a
# negative control that fails for an unrelated reason gets deleted, taking the
# proof with it.
rm -f "${WORK}/called"
env -u APEX_DISPLAY_NO_LIVE PATH="$FAKE" HOME="$H5" "$PY" "$GEN" apply >/dev/null 2>&1
mutating \
    && ok "without the guard, apply does mutate (so the guard is load-bearing)" \
    || bad "without the guard, apply does mutate (so the guard is load-bearing)"

section "live enumeration"
# Needs a real session. Reported honestly rather than stubbed: the whole point
# of enumeration is that it reflects hardware.
if [ -z "${WAYLAND_DISPLAY:-}" ]; then
    skp "no Wayland session; cannot enumerate real outputs"
elif ! command -v wlr-randr >/dev/null 2>&1 && ! command -v hyprctl >/dev/null 2>&1; then
    skp "neither wlr-randr nor hyprctl available"
else
    listed="$(python3 "$GEN" list 2>/dev/null)"
    if printf '%s' "$listed" | python3 -c "
import json,sys
o = json.load(sys.stdin)
assert isinstance(o, list) and o, 'no outputs'
for m in o:
    assert m.get('name'), 'an output has no name'
    assert isinstance(m.get('modes'), list), 'modes is not a list'
    assert m.get('transform') is not None
" 2>/dev/null; then
        n="$(printf '%s' "$listed" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)))')"
        ok "enumerated ${n} real output(s) with modes"
    else
        bad "enumeration returned usable output data"
    fi

    c="$(python3 "$GEN" compositor 2>/dev/null)"
    case "$c" in
        hyprland|niri|labwc) ok "the compositor is identified (${c})" ;;
        *) bad "the compositor is identified (got '${c}')" ;;
    esac
fi

printf '\napex-display: %d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
