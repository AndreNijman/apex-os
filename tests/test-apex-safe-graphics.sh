#!/usr/bin/env bash
#
# APEX-OS — the safe-graphics recovery session (roadmap P2-018).
#
# ── What this has to prove, and why the third one is the hard one ────────────
#
#   1. A graphics/compositor/shell failure can enter a conservative recovery
#      desktop.
#   2. From it, a user can inspect the failure, roll back, collect diagnostics
#      and reach files and network.
#   3. Recovery does not depend on the broken normal compositor configuration.
#
# The third is the one a structural test can get wrong. "It has its own config
# file" is not the criterion — the criterion is that the session does not READ
# the user's, and on this system that is not a hypothetical: APEX itself writes
# into `~/.config/labwc/rc.xml` (apex-input-apply splices a <libinput> element
# in with ElementTree; apex-labwc-keybinds generates the keybind block), and
# labwc falls back to its defaults SILENTLY on a malformed rc.xml. So a session
# that read the user's copy would inherit a broken one without saying so.
#
# The proof is therefore a measurement, not an inspection: plant a config that
# labwc cannot parse where the user's would be, start the session, and require
# it to come up anyway. `labwc -C <dir>` is what makes that true, and deleting
# the `-C` from the session script is the mutation that turns it red.
#
# ── Isolation: this suite starts a real compositor ───────────────────────────
# Andre is using this machine. The idiom is tests/test-labwc-keybind-reload.sh's
# verbatim: WLR_BACKENDS=headless, a private XDG_RUNTIME_DIR, WAYLAND_DISPLAY
# and DISPLAY unset, isolation asserted as HARD FAILURES before the compositor
# is started, and cleanup by pid — never `pkill labwc`, which would take the
# developer's own session with it.
#
# Requires labwc; SKIPs cleanly without it. Everything that is not the live
# launch is structural and runs anywhere.
#
# Usage: tests/test-apex-safe-graphics.sh

set -uo pipefail
# CI runs a script as `bash -e {0}`, under which `x="$(cmd)"` with a non-zero
# command kills the run part-way and reports the rest as failures. This suite
# counts failures instead of aborting on them.
set +e

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SESSION="${ROOT}/files/system/libexec/apex-safe-graphics"
TERMINAL="${ROOT}/files/system/libexec/apex-safe-graphics-terminal"
CONFIG="${ROOT}/files/desktop/safe-graphics"
ENTRY="${ROOT}/files/desktop/wayland-sessions/apex-safe-graphics.desktop"

pass=0
fail=0
skip=0
ok()   { printf 'PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad()  { printf 'FAIL  %s\n' "$1"; fail=$((fail + 1)); [ -n "${2:-}" ] && printf '      %s\n' "$2"; }
skp()  { printf 'SKIP  %s\n' "$1"; skip=$((skip + 1)); }
section() { printf '\n\033[1m── %s ──\033[0m\n' "$1"; }

finish() {
    printf '\napex-safe-graphics: %d passed, %d failed, %d skipped\n' \
        "$pass" "$fail" "$skip"
    [ "$fail" -eq 0 ]
}

# ── the files exist and parse ────────────────────────────────────────────────
section "what ships"

for f in "$SESSION" "$TERMINAL" "${CONFIG}/rc.xml" "${CONFIG}/menu.xml" \
         "${CONFIG}/autostart" "${CONFIG}/environment" "$ENTRY"; do
    [ -f "$f" ] && ok "$(basename "$f") ships" || bad "$(basename "$f") ships"
done
[ -x "$SESSION" ] && ok "the session helper is executable" \
    || bad "the session helper is executable"
[ -x "$TERMINAL" ] && ok "the terminal helper is executable" \
    || bad "the terminal helper is executable"

bash -n "$SESSION" 2>/dev/null && ok "apex-safe-graphics is valid bash" \
    || bad "apex-safe-graphics is valid bash"
bash -n "$TERMINAL" 2>/dev/null && ok "apex-safe-graphics-terminal is valid bash" \
    || bad "apex-safe-graphics-terminal is valid bash"
sh -n "${CONFIG}/autostart" 2>/dev/null && ok "the autostart is a valid sh script" \
    || bad "the autostart is a valid sh script"

if command -v xmllint >/dev/null 2>&1; then
    xmllint --noout "${CONFIG}/rc.xml" 2>/dev/null \
        && ok "rc.xml is well-formed XML" || bad "rc.xml is well-formed XML"
    xmllint --noout "${CONFIG}/menu.xml" 2>/dev/null \
        && ok "menu.xml is well-formed XML" || bad "menu.xml is well-formed XML"
else
    skp "xmllint is not installed; the XML is not checked here"
fi

# ── criterion 3, structurally ────────────────────────────────────────────────
section "independent of the user's configuration"

# The load-bearing flag. Asserted on the literal string rather than on "the
# script mentions the config dir", because `labwc` with the directory in an
# unrelated variable would satisfy the looser check and read ~/.config/labwc.
grep -q 'exec labwc -C "\$CONFIG_DIR"' "$SESSION" \
    && ok "the session execs labwc with a config DIRECTORY of its own" \
    || bad "the session execs labwc with a config DIRECTORY of its own" \
           "$(grep -n 'labwc' "$SESSION" | head -3)"

# `exec`, and nothing after it. greetd holds the compositor's own pid and
# signals it at logout; a wrapper left in the middle would hold the VT and DRM
# master on a machine nobody can log into. apex-greet-session carries the same
# rule and Containerfile.base asserts it there.
if grep -qE '^\s*labwc[^|]*&\s*$' "$SESSION"; then
    bad "the compositor is backgrounded rather than exec'd"
else
    ok "the compositor is not backgrounded"
fi

# APEX Shell is one of the things that can be broken. A recovery session that
# started it would fail the same way the session the user just left failed.
#
# Comment lines are stripped first, and that is not laxity: the autostart SAYS
# it deliberately does not run `apex-shell-autostart`, and a checker that read
# the sentence explaining the rule as a violation of it would train the next
# person to delete the explanation.
runnable() {
    grep -vE '^\s*(#|<!--|\s*-->)' "$@" 2>/dev/null | grep -v '^\s*$'
}
for forbidden in apex-shell-autostart "qs -p" quickshell Hyprland hyprland; do
    if runnable "${CONFIG}/autostart" "${CONFIG}/rc.xml" | grep -q -- "$forbidden"; then
        bad "the recovery session does not start $forbidden"
    else
        ok "the recovery session does not start $forbidden"
    fi
done

# ── software rendering, and the variable that must NOT be pinned ─────────────
section "software rendering"

for var in WLR_RENDERER=pixman LIBGL_ALWAYS_SOFTWARE=1 WLR_NO_HARDWARE_CURSORS=1; do
    grep -q "$var" "$SESSION" && ok "the session forces $var" \
        || bad "the session forces $var"
done

# WLR_BACKENDS pinned in the shipped path would mean this session could only
# ever be exercised by taking somebody's display away — and on a real machine
# `headless` would be a session with no screen. Unset is the only value that is
# right in both places.
if grep -qE '(export +)?WLR_BACKENDS=' "$SESSION" "${CONFIG}/environment"; then
    bad "WLR_BACKENDS is pinned, so this session cannot be tested headless"
else
    ok "WLR_BACKENDS is left unset, so a test may ask for a headless backend"
fi

# A broken GPU setup is often a broken vendor SELECTION, and those live in the
# environment the session inherits.
for var in __GLX_VENDOR_LIBRARY_NAME GBM_BACKEND MESA_LOADER_DRIVER_OVERRIDE; do
    grep -q "unset.*$var" "$SESSION" && ok "the session clears $var" \
        || bad "the session clears $var"
done

# ── criterion 2: the remedies are on the menu ────────────────────────────────
section "what a person can do from in here"

# `needle|description`, because two of the needles have spaces in them and a
# whitespace-split read would silently test the first word of each.
while IFS='|' read -r needle what; do
    [ -n "$needle" ] || continue
    grep -qF -- "$needle" "${CONFIG}/menu.xml" \
        && ok "the menu offers $what" \
        || bad "the menu offers $what" "nothing in menu.xml matches: $needle"
done <<'ENTRIES'
apex recover status|a way to inspect the failure
apex doctor|a full health report
apex-safe-graphics diagnose|collecting diagnostics
apex rollback|rolling the deployment back
thunar|reaching files
nmtui|reaching the network
Exit|leaving the session
ENTRIES

# A menu entry pointing at a binary the image does not ship is an entry that
# prints an error and does nothing, on the one screen where that matters most.
# Checked here rather than in Containerfile.base, and the reason is honest
# rather than tidy: this branch could not run an image build, so which LAYER
# provides thunar and NetworkManager-tui is unverified, and a build refusal
# placed before the layer that installs them would fail the build for the wrong
# reason. Both are present on the reference machine.
for bin in thunar nmtui; do
    if command -v "$bin" >/dev/null 2>&1; then
        ok "$bin, which the menu offers, is installed here"
    else
        skp "$bin is not installed here, so the menu entry naming it is unproven"
    fi
done

# The menu is on a plain right-click. The desktop session reserves that for
# APEX Shell's context menu and puts its emergency menu behind SUPER; there is
# no shell here, and a recovery menu reachable only by a chord is one somebody
# cannot find.
grep -q 'button="Right"' "${CONFIG}/rc.xml" \
    && grep -q 'ShowMenu' "${CONFIG}/rc.xml" \
    && ok "the root menu is on a plain right-click" \
    || bad "the root menu is on a plain right-click"

# ── the session entry the greeter reads ──────────────────────────────────────
section "the session entry"

grep -qx 'Exec=/usr/libexec/apex-safe-graphics' "$ENTRY" \
    && ok "the entry execs the session helper" || bad "the entry execs the session helper"
grep -qx 'TryExec=/usr/libexec/apex-safe-graphics' "$ENTRY" \
    && ok "the entry carries TryExec, so a build that lost the helper hides it" \
    || bad "the entry carries TryExec"

# Containerfile.apex fails the build if any shipped session Name= carries an
# implementation token, and tests/test-apex-greet-sessions.sh asserts the same
# list. Checked here too, because this is the file that would carry one.
name="$(grep -m1 '^Name=' "$ENTRY" | cut -d= -f2-)"
lower="$(printf '%s' "$name" | tr '[:upper:]' '[:lower:]')"
token_found=""
for token in labwc hyprland niri sway wlroots openbox gamescope wayfire river; do
    case "$lower" in *"$token"*) token_found="$token" ;; esac
done
[ -z "$token_found" ] && ok "the session name is a product name ($name)" \
    || bad "the session name carries the implementation token '$token_found'"

# ── verbs ────────────────────────────────────────────────────────────────────
section "verbs"

out="$("$SESSION" nonsense 2>&1)"
rc=$?
[ "$rc" -eq 2 ] && printf '%s' "$out" | grep -q "is not a verb" \
    && ok "an unknown verb is refused and says what the verbs are" \
    || bad "an unknown verb is refused" "rc=$rc: $out"

# `check` against a config directory that is not there must FAIL rather than
# report a session that would not start. "Permission denied is not absence" —
# and neither is "the directory is missing" the same answer as "it is fine".
out="$(APEX_SAFE_GRAPHICS_CONFIG=/nonexistent-safe-graphics "$SESSION" check 2>&1)"
rc=$?
[ "$rc" -ne 0 ] && ok "check fails when the config directory is not there" \
    || bad "check fails when the config directory is not there" "$out"

if command -v labwc >/dev/null 2>&1; then
    out="$(APEX_SAFE_GRAPHICS_CONFIG="$CONFIG" "$SESSION" check 2>&1)"
    rc=$?
    [ "$rc" -eq 0 ] && printf '%s' "$out" | grep -q 'pixman' \
        && ok "check passes against the shipped config and names the renderer" \
        || bad "check passes against the shipped config" "rc=$rc: $out"
else
    skp "labwc is not installed; check cannot pass here"
fi

# ── the live half ────────────────────────────────────────────────────────────
section "it comes up over a broken user configuration"

if ! command -v labwc >/dev/null 2>&1; then
    skp "labwc is not installed; the session cannot be started here"
    finish; exit 0
fi

# Saved BEFORE they are unset, so the assertions compare against the real
# ambient values rather than against emptiness.
AMBIENT_WL="${WAYLAND_DISPLAY:-}"
AMBIENT_RT="${XDG_RUNTIME_DIR:-}"
# `labwc --reconfigure` SIGHUPs whatever this names. Nothing here reconfigures,
# and it is cleared anyway: an inherited value is a signal aimed at the
# developer's own session.
unset LABWC_PID
unset WAYLAND_DISPLAY
unset DISPLAY

WORK="$(mktemp -d)"
COMP_PID=""
cleanup() {
    # By pid, always. `pkill labwc` would kill the developer's session.
    [ -n "$COMP_PID" ] && kill "$COMP_PID" 2>/dev/null
    [ -n "$COMP_PID" ] && wait "$COMP_PID" 2>/dev/null
    rm -rf "$WORK"
}
trap cleanup EXIT INT TERM

export XDG_RUNTIME_DIR="${WORK}/rt"
mkdir -p "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
export HOME="${WORK}/home"
export XDG_CONFIG_HOME="${HOME}/.config"
mkdir -p "${XDG_CONFIG_HOME}/labwc" "${XDG_CONFIG_HOME}/hypr/apex"

# THE MEASUREMENT. Not "a different config file" — a config file labwc cannot
# parse, exactly where the user's lives, with an autostart that would take the
# session down if it were read. If the recovery session read any of this, it
# would come up with labwc's silent defaults and this suite would still see a
# socket; what it would NOT survive is the autostart, which is why one is here.
cat > "${XDG_CONFIG_HOME}/labwc/rc.xml" <<'BROKEN'
<?xml version="1.0"?>
<labwc_config>
  <core><gap>not-a-number</gap>
  <!-- deliberately unclosed, and with a keybind element that is not one -->
  <keyboard><keybind key=""><action name="NoSuchAction"/></keybind>
BROKEN
cat > "${XDG_CONFIG_HOME}/labwc/autostart" <<'BROKEN'
#!/bin/sh
# If the recovery session reads the user's config directory, this runs and the
# compositor goes away — which is precisely the failure being ruled out.
labwc -e 2>/dev/null || true
exit 1
BROKEN
cat > "${XDG_CONFIG_HOME}/hypr/apex/monitors.lua" <<'BROKEN'
this is not lua and never was
BROKEN
ok "a config the compositor cannot parse is planted where the user's lives"

# A terminal stub, so the autostart is observable without a real terminal and
# without anything opening on a display.
mkdir -p "${WORK}/bin"
STARTED="${WORK}/terminal-started"
cat > "${WORK}/bin/apex-fixture-terminal" <<STUB
#!/bin/sh
echo started >> "${STARTED}"
sleep 300
STUB
chmod +x "${WORK}/bin/apex-fixture-terminal"
export PATH="${WORK}/bin:${PATH}"
export APEX_SAFE_GRAPHICS_TERMINAL=apex-fixture-terminal

# The backend the shipped session deliberately does not pin. Set HERE, by the
# test, which is the whole reason it is not set there.
export WLR_BACKENDS=headless
export WLR_HEADLESS_OUTPUTS=1

APEX_SAFE_GRAPHICS_CONFIG="$CONFIG" "$SESSION" > "${WORK}/comp.log" 2>&1 &
COMP_PID=$!

sock=""
for _ in $(seq 1 80); do
    # A glob and a socket test, not `ls | grep`: the lock file beside the
    # socket is named wayland-N.lock and matches the same pattern.
    for cand in "$XDG_RUNTIME_DIR"/wayland-[0-9]*; do
        [ -S "$cand" ] && { sock="${cand##*/}"; break; }
    done
    [ -n "$sock" ] && break
    sleep 0.2
done

if [ -z "$sock" ]; then
    bad "the recovery session came up over a broken user configuration" \
        "$(tail -8 "${WORK}/comp.log" 2>/dev/null)"
    finish; exit 1
fi
ok "the recovery session came up over a broken user configuration"

# Three hard failures, not skips. Each is a way this could have attached to the
# developer's own desktop instead of to a compositor of its own.
[ -S "${XDG_RUNTIME_DIR}/${sock}" ] \
    && ok "the wayland socket is inside this suite's own runtime dir" \
    || bad "the wayland socket is inside this suite's own runtime dir"
if [ -n "$AMBIENT_WL" ] && [ "$sock" = "$AMBIENT_WL" ]; then
    bad "this display is not the ambient one [$AMBIENT_WL]"
else
    ok "this display ($sock) is not the ambient one (${AMBIENT_WL:-none})"
fi
if [ -n "$AMBIENT_RT" ] && [ "$XDG_RUNTIME_DIR" = "$AMBIENT_RT" ]; then
    bad "this runtime dir is not the ambient one [$AMBIENT_RT]"
else
    ok "this runtime dir is not the ambient one"
fi

# The autostart that ran is the recovery session's, not the user's: the
# fixture terminal started, and the user's autostart would have exited the
# compositor instead.
started=""
for _ in $(seq 1 40); do
    [ -s "$STARTED" ] && { started=yes; break; }
    sleep 0.2
done
[ -n "$started" ] && ok "the recovery session's own autostart ran, and opened a terminal" \
    || bad "the recovery session's own autostart ran" "$(tail -5 "${WORK}/comp.log")"

kill -0 "$COMP_PID" 2>/dev/null \
    && ok "the compositor is still up after the user's autostart would have exited it" \
    || bad "the compositor is still up"

# ── diagnostics ──────────────────────────────────────────────────────────────
section "diagnostics"

bundle_dir="${WORK}/diag"
mkdir -p "$bundle_dir"
archive="$("$SESSION" diagnose "$bundle_dir" 2>/dev/null | tail -1)"
if [ -n "$archive" ] && [ -f "$archive" ]; then
    ok "diagnose writes an archive and prints where it is"
    listing="$(tar -tzf "$archive" 2>/dev/null)"
    for member in boot.log recover.json doctor.json graphics.txt modules.txt; do
        printf '%s' "$listing" | grep -q "$member" \
            && ok "the bundle carries $member" || bad "the bundle carries $member"
    done
    # "Permission denied is not absence": a step that could not run must say so
    # IN its file rather than leaving an empty one that reads as "nothing to
    # report". At least one of these is normally unrunnable in a test
    # environment, so this is checked over the whole bundle rather than over a
    # named member.
    extracted="${WORK}/unpacked"
    mkdir -p "$extracted"
    tar -xzf "$archive" -C "$extracted" 2>/dev/null
    empty=""
    for f in "$extracted"/*; do
        [ -f "$f" ] || continue
        [ -s "$f" ] || empty="$empty $(basename "$f")"
    done
    [ -z "$empty" ] && ok "no file in the bundle is empty; an unrunnable step says so" \
        || bad "empty files in the bundle read as 'nothing to report':$empty"
else
    bad "diagnose writes an archive and prints where it is"
fi

out="$("$SESSION" diagnose /nonexistent-directory 2>&1)"
rc=$?
[ "$rc" -ne 0 ] && printf '%s' "$out" | grep -q 'FATAL' \
    && ok "diagnose refuses a destination it cannot write to" \
    || bad "diagnose refuses a destination it cannot write to" "rc=$rc: $out"

finish
