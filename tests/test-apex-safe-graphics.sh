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
# The SECOND is the one this suite got wrong for longer. Naming the remedies on
# the menu, refusing the build when one of their binaries is missing and
# asserting those binaries exist is a statement about the image, not about the
# session: none of it had ever RUN anything from inside the session. The
# section "the remedies run from inside the session" closes that. It reads the
# command lines out of menu.xml, replays the environment the compositor handed
# a client it started itself, and requires each remedy to answer — with the
# three that cannot be executed here (a privileged rollback, a full-screen TUI
# with no terminal, and labwc's own Exit action) told apart from a remedy that
# is simply broken, one by one and with the reason on the line.
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
# Captured to a variable and matched with `[[ == * * ]]`, NOT piped into
# `grep -q`. Under `set -o pipefail` a pipeline whose reader exits on the first
# match leaves the writer holding a closed pipe: grep -q returns 0, the writer
# dies of SIGPIPE, and the PIPELINE returns 141. `if … ; then bad` would then
# take the `else` branch — reporting "the recovery session does not start
# quickshell" at precisely the moment quickshell is in the file. Whether the
# writer gets that far before grep exits is decided by how big these two files
# happen to be, which is not a property any assertion should rest on. Three
# negative leak assertions in this repository reported clean for exactly this
# reason.
runnable_body="$(runnable "${CONFIG}/autostart" "${CONFIG}/rc.xml")"
for forbidden in apex-shell-autostart "qs -p" quickshell Hyprland hyprland; do
    if [[ "$runnable_body" == *"$forbidden"* ]]; then
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
[ "$rc" -eq 2 ] && [[ "$out" == *"is not a verb"* ]] \
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
    [ "$rc" -eq 0 ] && [[ "$out" == *pixman* ]] \
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
AMBIENT_X="${DISPLAY:-}"
AMBIENT_BUS="${DBUS_SESSION_BUS_ADDRESS:-}"
# `labwc --reconfigure` SIGHUPs whatever this names. Nothing here reconfigures,
# and it is cleared anyway: an inherited value is a signal aimed at the
# developer's own session.
unset LABWC_PID
unset WAYLAND_DISPLAY
unset DISPLAY
# The session bus, and this one is load-bearing rather than tidy. The remedies
# below include a GUI file manager, and a GUI application on the DEVELOPER's
# session bus does not open a window of its own: it hands its arguments to the
# instance already running there and a window appears on his desktop. Clearing
# it is half the guard; the other half is that the file manager is started
# under `dbus-run-session`, on a bus that exists for the length of that one
# launch, and that this suite refuses to start it at all if the environment the
# compositor hands its clients still carries a bus address.
unset DBUS_SESSION_BUS_ADDRESS
unset DBUS_SESSION_BUS_PID

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
CLIENT_ENV="${WORK}/client.env"
# `env -0` BEFORE the marker, so that a marker implies a COMPLETE dump. The
# other order would let the assertions below read a half-written file on a slow
# machine and blame the session for it.
#
# This dump is the whole reason the remedies further down can claim to run
# "from inside the session": it is the environment the compositor actually
# handed a client it started itself, not an environment this suite assembled
# from what it believes the session exports.
cat > "${WORK}/bin/apex-fixture-terminal" <<STUB
#!/bin/sh
env -0 > "${CLIENT_ENV}"
echo started >> "${STARTED}"
sleep 300
STUB
chmod +x "${WORK}/bin/apex-fixture-terminal"

# The terminal `apex-safe-graphics-terminal` dispatches to under its FOOT
# convention — the command positional, with no `-e`. Named `foot` because the
# wrapper decides by `basename`, so this is the real branch and not a
# lookalike. Handed to the wrapper by absolute path, so nothing depends on
# which `foot` PATH would have found.
cat > "${WORK}/bin/foot" <<'STUB'
#!/bin/sh
if [ "$#" -eq 0 ]; then
    printf 'apex-fixture-foot: a terminal with no command\n'
    exit 0
fi
exec "$@"
STUB
chmod +x "${WORK}/bin/foot"

# The tripwire. `sudo apex rollback` is on the recovery menu and it reboots
# into another deployment; `pkexec` is the other way to the same prompt. These
# turn "the suite does not run the privileged entry" from a claim into an
# assertion — and they are on the PATH the compositor hands its clients, so
# they cover the remedies too, not just this shell.
TRIPWIRE="${WORK}/tripwire.log"
for guard in sudo pkexec; do
    cat > "${WORK}/bin/${guard}" <<STUB
#!/bin/sh
printf '%s %s\n' "${guard}" "\$*" >> "${TRIPWIRE}"
exit 99
STUB
    chmod +x "${WORK}/bin/${guard}"
done

# The menu names `apex`, so the remedies must resolve it through PATH the way
# the menu does. WHICH binary that is gets printed rather than assumed: a suite
# that quietly measured an installed /usr/bin/apex would be reporting the
# shipped CLI's behaviour as this branch's.
APEX_BIN="${APEX_BIN:-}"
if [ -z "$APEX_BIN" ] && [ -x "${ROOT}/apexd/target/debug/apex" ]; then
    APEX_BIN="${ROOT}/apexd/target/debug/apex"
fi
[ -n "$APEX_BIN" ] || APEX_BIN="$(command -v apex 2>/dev/null)"
[ -n "$APEX_BIN" ] && ln -sf "$APEX_BIN" "${WORK}/bin/apex"

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

# ── criterion 2, live: the remedies are RUN from inside the session ──────────
section "the remedies run from inside the session"

# Everything above proves the menu NAMES the remedies and that their binaries
# exist. That is not the criterion. The criterion is that somebody sitting in
# this session can inspect the failure, roll the machine back, collect
# diagnostics and reach files and network — and not one of those had ever been
# EXECUTED from inside the session.
#
# Two things stop "from inside" meaning "in the test's own shell with a
# variable set":
#
#   1. The environment is MEASURED. The fixture terminal that the session's own
#      autostart starts dumped its environment with `env -0`, and every remedy
#      below runs under `env -i` with exactly those pairs. It is the
#      environment the compositor handed a client it started itself, not one
#      this suite assembled from what it believes the session exports.
#   2. The command lines are READ OUT OF menu.xml. A remedy renamed in the menu
#      and not here would otherwise go on being tested in its old spelling for
#      as long as the file exists.
#
# Three things are deliberately NOT run, and each is a different answer from
# "broken":
#
#   * `sudo apex rollback` — it execs `bootc rollback` on the machine running
#     the suite and it needs the authentication prompt this project has asked
#     twice never to see. Its verb is asked for `--help` instead, in the
#     session's environment, and the `sudo`/`pkexec` tripwire on the session's
#     own PATH turns "we did not run it" into an assertion rather than a claim.
#   * `nmtui`'s full-screen interface — it needs a terminal, and driving it
#     means driving NetworkManager's saved connections. What IS measured is the
#     thing that makes the menu's wrapper load-bearing: with no terminal nmtui
#     exits ZERO having done nothing, so "run it and check the status" would
#     report the network remedy working when it never drew a line.
#   * the `Exit` entry — labwc's own action rather than a command, and running
#     it would end the session the rest of these assertions are standing in.

CLIENT_KV=()
[ -s "$CLIENT_ENV" ] && mapfile -d '' CLIENT_KV < "$CLIENT_ENV"

# `$key=` prefix match over the dump. Returns 0 for a variable that is set and
# empty, which is a different fact from one that is absent — the distinction
# the isolation gate below rests on.
client_value() {
    local key="$1" kv
    for kv in ${CLIENT_KV+"${CLIENT_KV[@]}"}; do
        case "$kv" in "$key"=*) printf '%s' "${kv#*=}"; return 0 ;; esac
    done
    return 1
}
client_has_key() { client_value "$1" >/dev/null 2>&1; }

if [ "${#CLIENT_KV[@]}" -eq 0 ]; then
    bad "the environment the session hands its clients was captured" \
        "no dump at $CLIENT_ENV"
else
    ok "the environment the session hands its clients was captured (${#CLIENT_KV[@]} variables)"
fi

if [ "${#CLIENT_KV[@]}" -gt 0 ]; then

# THIS compositor, not one. The suite already knew the stub had run; it did not
# know what the stub was connected to, and a client that inherited the
# developer's WAYLAND_DISPLAY would have started, written the marker and proved
# nothing at all.
client_wl="$(client_value WAYLAND_DISPLAY)"
[ "$client_wl" = "$sock" ] \
    && ok "a client the recovery session started talks to THIS compositor ($sock)" \
    || bad "a client the recovery session started talks to THIS compositor" \
           "WAYLAND_DISPLAY=${client_wl:-<unset>}, this session's socket is $sock"

client_desktop="$(client_value XDG_CURRENT_DESKTOP)"
[ "$client_desktop" = labwc ] \
    && ok "the session identifies itself to its clients (XDG_CURRENT_DESKTOP=labwc)" \
    || bad "the session identifies itself to its clients" \
           "XDG_CURRENT_DESKTOP=${client_desktop:-<unset>}"

# Software rendering has to reach the CLIENTS, not only the compositor: the
# remedy on this menu that draws is a GTK application, and a GTK application
# that asks for hardware GL on the machine this session exists for gets a blank
# window or a crash. Two independent paths put these here — the session script
# exports them before it execs labwc, and `environment` in the config directory
# is read by labwc and applied to what it starts — and that duplication is
# deliberate and documented in `environment` itself. This asserts the OUTCOME,
# so it is satisfied by either path and lost only if both go.
for pair in WLR_RENDERER=pixman LIBGL_ALWAYS_SOFTWARE=1 \
            WLR_NO_HARDWARE_CURSORS=1 GALLIUM_DRIVER=llvmpipe \
            QT_QUICK_BACKEND=software; do
    key="${pair%%=*}"
    want="${pair#*=}"
    got="$(client_value "$key")"
    [ "$got" = "$want" ] \
        && ok "software rendering reaches the session's clients: $pair" \
        || bad "software rendering reaches the session's clients: $pair" \
               "got ${got:-<unset>}"
done

# The autostart and the menu's terminal wrapper have to agree about which
# terminal this is, and they agree through the environment rather than through
# two copies of the same search order.
client_term="$(client_value APEX_SAFE_GRAPHICS_TERMINAL)"
[ -n "$client_term" ] \
    && ok "the terminal the session chose reaches its clients ($client_term), so the menu's wrapper opens the same one the autostart did" \
    || bad "the terminal the session chose reaches its clients" \
           "APEX_SAFE_GRAPHICS_TERMINAL is not in the client environment"

# The documented asymmetry, asserted rather than described: the COMPOSITOR is
# isolated from the user's configuration (`labwc -C`), and the CLIENTS are
# deliberately not — a file manager with no config directory cannot save a
# bookmark, and somebody in recovery came here to reach their home directory.
client_cfg="$(client_value XDG_CONFIG_HOME)"
[ "$client_cfg" = "$XDG_CONFIG_HOME" ] \
    && ok "clients still see the user's own config home, which is the documented half of the isolation" \
    || bad "clients still see the user's own config home" \
           "XDG_CONFIG_HOME=${client_cfg:-<unset>} rather than $XDG_CONFIG_HOME"

# ── the gate that decides whether a GUI remedy may be started at all ─────────
# labwc runs an XWayland of its own and hands its clients a DISPLAY for it, so
# "no DISPLAY" is not the check — "not the developer's DISPLAY" is. A session
# bus address is checked for ABSENCE, because a GUI application on a bus that
# already has an instance of it does not open a window: it hands its arguments
# to that instance, and the window appears on the desktop of whoever owns the
# bus.
GUI_SAFE=yes
GUI_WHY=""
client_x="$(client_value DISPLAY)"
if [ -n "$AMBIENT_X" ] && [ "$client_x" = "$AMBIENT_X" ]; then
    GUI_SAFE=""
    GUI_WHY="the client DISPLAY is the ambient one ($AMBIENT_X)"
fi
if client_has_key DBUS_SESSION_BUS_ADDRESS; then
    GUI_SAFE=""
    GUI_WHY="${GUI_WHY:+$GUI_WHY; }the client environment carries a session bus address ($(client_value DBUS_SESSION_BUS_ADDRESS))"
fi
if [ -n "$GUI_SAFE" ]; then
    ok "a GUI remedy started here cannot reach the developer's desktop (X display ${client_x:-<unset>} vs ${AMBIENT_X:-none}, no session bus inherited from ${AMBIENT_BUS:-none})"
else
    bad "a GUI remedy started here cannot reach the developer's desktop" "$GUI_WHY"
fi

# ── running things the way the session's clients run them ────────────────────
# `env -i` and not `env`: anything this shell holds that the compositor did not
# hand down would be a difference between what is measured and what a person in
# the session gets. The terminal is overridden to the fixture that execs its
# argument, because the wrapper's job is the dispatch and a real terminal here
# would be a window nobody can read.
in_session() {
    local secs="$1"
    shift
    env -i "${CLIENT_KV[@]}" \
        APEX_SAFE_GRAPHICS_TERMINAL="${WORK}/bin/foot" \
        timeout -k 5 "$secs" "$@" 2>&1
}

# "Is it installed" asked in the session's PATH rather than in this shell's.
in_session_have() {
    env -i "${CLIENT_KV[@]}" \
        sh -c 'command -v "$1" >/dev/null 2>&1' sh "$1"
}

if [ -n "$APEX_BIN" ]; then
    ok "the CLI the menu names resolves in the session ($APEX_BIN)"
else
    skp "no apex binary was found (set APEX_BIN, or build apexd/target/debug/apex); the CLI remedies cannot be run here"
fi

# The command lines, read out of the menu. Substitution is longest-first: the
# terminal wrapper's path has the session helper's path as a prefix, and the
# other order would rewrite `apex-safe-graphics-terminal` into
# `<repo>/files/system/libexec/apex-safe-graphics-terminal` with the wrong half
# replaced.
menu_lines="$(grep -o 'command="[^"]*"' "${CONFIG}/menu.xml" | sed 's/^command="//; s/"$//')"
menu_count="$(printf '%s\n' "$menu_lines" | grep -c '[^[:space:]]')"
covered=0

while IFS= read -r raw; do
    [ -n "${raw//[[:space:]]/}" ] || continue
    cmd="$(printf '%s' "$raw" \
        | sed -e "s#/usr/libexec/apex-safe-graphics-terminal#${TERMINAL}#g" \
              -e "s#/usr/libexec/apex-safe-graphics#${SESSION}#g")"
    read -r -a argv <<< "$cmd"
    if [ "${argv[0]}" = "$TERMINAL" ]; then
        wrapped=yes
        remedy=("${argv[@]:1}")
    else
        wrapped=""
        remedy=("${argv[@]}")
    fi
    label="${remedy[*]}"

    # An entry the menu does NOT put in a terminal is a graphical client. That
    # is the menu's own distinction rather than a list retyped here, so an
    # entry added without a terminal gets treated as graphical automatically.
    if [ -z "$wrapped" ]; then
        bin="${remedy[0]}"
        if ! in_session_have "$bin"; then
            skp "$bin is not installed in the session's PATH, so the menu's graphical entry is unproven live"
        elif [ -z "$GUI_SAFE" ]; then
            bad "$bin was NOT started: the session's client environment is not isolated from the developer's" "$GUI_WHY"
        elif ! command -v dbus-run-session >/dev/null 2>&1; then
            skp "dbus-run-session is not installed, so $bin is not started here — on a bus it did not own, a file manager hands its window to the instance already running on the developer's desktop"
        else
            # ALIVE-AT-THE-DEADLINE is the measurement. `timeout` returns 124
            # only for a process that was still running when the clock ran out,
            # and with GDK_BACKEND=wayland and no compositor to reach, a GTK
            # application exits in milliseconds. The control below is the
            # mutation, run every time rather than once by hand: the same
            # launch pointed at a socket that does not exist must NOT survive,
            # or 124 is a number anything would score.
            env -i "${CLIENT_KV[@]}" GDK_BACKEND=wayland \
                dbus-run-session -- timeout -k 2 6 "$bin" \
                > "${WORK}/gui-${bin}.log" 2>&1
            grc=$?
            [ "$grc" -eq 124 ] \
                && ok "$bin, the menu's graphical entry, connects to the recovery session and stays up" \
                || bad "$bin, the menu's graphical entry, connects to the recovery session and stays up" \
                       "exited on its own, rc=$grc: $(tail -3 "${WORK}/gui-${bin}.log")"

            env -i "${CLIENT_KV[@]}" GDK_BACKEND=wayland \
                WAYLAND_DISPLAY=apex-no-such-socket \
                dbus-run-session -- timeout -k 2 6 "$bin" \
                > "${WORK}/gui-${bin}-control.log" 2>&1
            crc=$?
            [ "$crc" -ne 124 ] \
                && ok "the same launch with no compositor to reach exits instead (rc=$crc), so staying up is a measurement" \
                || bad "the control survived too, so staying up measures nothing" \
                       "$(tail -3 "${WORK}/gui-${bin}-control.log")"

            kill -0 "$COMP_PID" 2>/dev/null \
                && ok "the compositor survived its graphical client" \
                || bad "the compositor survived its graphical client"
        fi
        covered=$((covered + 1))
        continue
    fi

    case "${remedy[0]:-}" in
    "")
        # The bare Terminal entry: the wrapper with no command at all. The
        # fixture prints and exits 0; the wrapper reaching it through the FOOT
        # branch is what is being measured, and a wrapper that passed `-e` here
        # would make the fixture try to exec `-e`.
        out="$(in_session 20 "$TERMINAL")"
        rc=$?
        [ "$rc" -eq 0 ] && [ -n "$out" ] \
            && ok "the menu's terminal entry opens a terminal from inside the session" \
            || bad "the menu's terminal entry opens a terminal from inside the session" \
                   "rc=$rc: $out"
        covered=$((covered + 1))
        ;;
    sudo)
        # NEVER RUN. `apex rollback` execs `bootc rollback` on this machine.
        # What is run is the same verb's `--help`, which clap answers during
        # parsing and before any of the command's own code — so the menu's
        # entry is proved to name a verb the shipped CLI has, without the
        # deployment changing and without a prompt.
        verb=("${remedy[@]:1}")
        if ! in_session_have "${verb[0]}"; then
            skp "${verb[0]} is not installed in the session's PATH, so the privileged entry (${label}) is unproven live"
        else
            out="$(in_session 30 "${verb[@]}" --help)"
            rc=$?
            [ "$rc" -eq 0 ] && [ -n "$out" ] \
                && ok "the privileged entry's verb (${verb[*]}) is one the CLI has, asked from inside the session and never run" \
                || bad "the privileged entry's verb (${verb[*]}) is one the CLI has" \
                       "rc=$rc: $out"
        fi
        covered=$((covered + 1))
        ;;
    nmtui)
        if ! in_session_have nmtui; then
            skp "nmtui is not installed in the session's PATH, so the network remedy is unproven live"
        else
            out="$(in_session 30 "$TERMINAL" nmtui --help)"
            rc=$?
            [ "$rc" -eq 0 ] && [ -n "$out" ] \
                && ok "the network remedy runs from inside the session through the menu's own wrapper" \
                || bad "the network remedy runs from inside the session" "rc=$rc: $out"

            # THE FINDING that makes the wrapper load-bearing, measured rather
            # than assumed: nmtui with no terminal exits ZERO. A suite that
            # ran the remedy and checked the status would call the network
            # remedy healthy on a session where it drew nothing at all.
            out="$(in_session 20 nmtui < /dev/null)"
            rc=$?
            if [ "$rc" -ne 0 ] || [ -n "$out" ]; then
                ok "nmtui with no terminal does not succeed silently, which is why the menu gives it one"
            else
                bad "nmtui with no terminal exited 0 with nothing to say" \
                    "a user would get an empty window and no way to tell"
            fi
        fi
        covered=$((covered + 1))
        ;;
    "$SESSION")
        # As the menu runs it: no destination argument, so the bundle lands in
        # the HOME the session handed its clients.
        out="$(in_session 180 "$TERMINAL" "${remedy[@]}")"
        rc=$?
        archive="$(printf '%s\n' "$out" | tail -1)"
        if [ "$rc" -eq 0 ] && [ -n "$archive" ] && [ -f "$archive" ]; then
            ok "the diagnostics remedy writes a bundle from inside the session"
            comp="$(tar -xzOf "$archive" ./compositor.txt 2>/dev/null)"
            if [[ "$comp" == *"WAYLAND_DISPLAY=${sock}"* ]] \
               && [[ "$comp" == *"XDG_CURRENT_DESKTOP=labwc"* ]]; then
                ok "the bundle describes the recovery session it was collected from, not the session that broke"
            else
                bad "the bundle describes the recovery session it was collected from" \
                    "compositor.txt: ${comp:-<absent>}"
            fi
        else
            bad "the diagnostics remedy writes a bundle from inside the session" \
                "rc=$rc: $out"
        fi
        covered=$((covered + 1))
        ;;
    *)
        # The text remedies. `apex recover status` exits 1 when a component
        # needs attention (recover.rs) — on a machine with something wrong,
        # which is the machine this session exists for, a non-zero status is
        # the remedy WORKING. Treating it as failure is how a recovery suite
        # comes to be green only on healthy hardware. Anything else has to
        # exit 0; a new menu entry that legitimately does not is a decision
        # somebody should have to write down here.
        if ! in_session_have "${remedy[0]}"; then
            skp "${remedy[0]} is not installed in the session's PATH, so '${label}' is unproven live"
        else
            case "$label" in
            "apex recover status") accept="0 1" ;;
            *) accept="0" ;;
            esac
            out="$(in_session 120 "$TERMINAL" "${remedy[@]}")"
            rc=$?
            verdict=""
            for good in $accept; do [ "$rc" = "$good" ] && verdict=ok; done
            if [ "$rc" -eq 124 ]; then
                bad "'${label}' answers from inside the session" \
                    "it did not finish; a recovery screen cannot wait on it"
            elif [ -n "$verdict" ] && [ -n "$out" ]; then
                ok "'${label}' answers from inside the session (rc=$rc, $(printf '%s' "$out" | wc -l) lines)"
            elif [ -n "$verdict" ]; then
                bad "'${label}' answers from inside the session" \
                    "rc=$rc and it printed nothing, which on a recovery screen is indistinguishable from not having run"
            else
                bad "'${label}' answers from inside the session" \
                    "rc=$rc, accepted: $accept — $(printf '%s' "$out" | tail -3)"
            fi
        fi
        covered=$((covered + 1))
        ;;
    esac
done <<< "$menu_lines"

[ "$covered" -eq "$menu_count" ] \
    && ok "every command on the recovery menu got a verdict ($covered of $menu_count)" \
    || bad "every command on the recovery menu got a verdict" \
           "$covered of $menu_count; an entry was added to menu.xml that nothing here classifies"

# The tripwire, read after everything above has run. `sudo apex rollback` is on
# this menu and nothing here may have reached it — not the wrapper, not a
# remedy that shells out, not a future entry.
if [ -s "$TRIPWIRE" ]; then
    bad "nothing in this suite asked for privilege" "$(cat "$TRIPWIRE")"
else
    ok "nothing in this suite asked for privilege: the sudo and pkexec tripwires on the session's PATH never fired"
fi

fi  # the client environment was captured

# ── diagnostics ──────────────────────────────────────────────────────────────
section "diagnostics"

bundle_dir="${WORK}/diag"
mkdir -p "$bundle_dir"
archive="$("$SESSION" diagnose "$bundle_dir" 2>/dev/null | tail -1)"
if [ -n "$archive" ] && [ -f "$archive" ]; then
    ok "diagnose writes an archive and prints where it is"
    listing="$(tar -tzf "$archive" 2>/dev/null)"
    for member in boot.log recover.json doctor.json graphics.txt modules.txt; do
        [[ "$listing" == *"$member"* ]] \
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
[ "$rc" -ne 0 ] && [[ "$out" == *FATAL* ]] \
    && ok "diagnose refuses a destination it cannot write to" \
    || bad "diagnose refuses a destination it cannot write to" "rc=$rc: $out"

finish
