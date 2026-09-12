#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-greet-session-bus.sh — the greeter's accessibility markup is
#  correct and a screen reader still receives nothing, because the session the
#  greeter runs in has no D-Bus session bus for the bridge to publish on
#  (roadmap P2-003, "screen reader … validated").
#
#  ── Why this suite exists ───────────────────────────────────────────────────
#
#  tests/test-apex-greet-atspi.sh proved the greeter's tree is readable over
#  AT-SPI: 30 assertions, every one of them against a real bus. Its mutant A7
#  then proved the other half — take the SESSION bus away from the application
#  and it publishes ZERO nodes, because Qt's accessibility bridge resolves
#  org.a11y.Bus on the session bus like every other desktop application.
#
#  A7 is the condition the shipped greeter runs in. This suite is the config
#  half of that finding: it takes the command the image really runs and RUNS
#  it, with the compositor and quickshell replaced by stubs, and asks the
#  greeter's own client what it can see. Nothing here reads a screen or needs a
#  GPU; the answer is decided entirely by what the chain exports.
#
#  ── A statement of current state, written to FLIP ───────────────────────────
#
#  Every assertion below records a hole. When somebody closes it — a session bus
#  in the greeter chain, and an at-spi bus launcher execed alongside it — this
#  suite goes RED and names itself, rather than quietly continuing to pass while
#  its header describes a world that no longer exists. The failure messages say
#  so. If you are reading one of them because you just fixed the greeter: update
#  the P2-003 "screen reader" ledger row in ROADMAP/state/agents/p2-b.md and
#  invert the assertion, do not delete it.
#
#  ── Why a session bus alone is not the fix ──────────────────────────────────
#
#  Measured, and printed as a NOTE below rather than asserted, because it is a
#  property of the machine rather than of this repository: on an SELinux system
#  org.a11y.Bus cannot be D-Bus ACTIVATED at all — at-spi-bus-launcher is
#  refused with EACCES and no AVC is logged. A byte-identical copy of the same
#  binary under a different label activates perfectly. So `dbus-run-session` in
#  front of sway would give the bridge a bus and still no accessibility bus.
#  tests/lib/atspi.sh execs the launcher for exactly this reason.
#
#  ── What it will not do ─────────────────────────────────────────────────────
#
#  It starts no compositor, opens no window, touches no display and reaches no
#  bus belonging to whoever is sitting at the machine. `sway`, `labwc`, `qs`,
#  `swaymsg` and `pkill` are all stubs on a PATH private to this run — `pkill`
#  most of all, because the labwc fallback's autostart ends in
#  `pkill -TERM -x labwc` and this suite is not permitted to send that signal to
#  anything real. Every run is under `env -i` with a private XDG_RUNTIME_DIR.
#
#  Run from anywhere: ./tests/test-apex-greet-session-bus.sh
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
# Deliberately +e, like the other suites here: CI invokes a suite as
# `bash -e {0}`, and under -e an assignment from a failing command ends the run
# silently, mid-section.
set +e

cd "$(dirname "$0")" || exit 2
ROOT="$(cd .. && pwd)"
GREETD_TOML="$ROOT/files/desktop/apex-greet/greetd-config.toml"
SWAY_CONF="$ROOT/files/desktop/apex-greet/sway-greet.conf"
LABWC_AUTOSTART="$ROOT/files/desktop/apex-greet/labwc-greet/autostart"
CF="$ROOT/Containerfile.base"
for f in "$GREETD_TOML" "$SWAY_CONF" "$LABWC_AUTOSTART" "$CF"; do
    [ -f "$f" ] || { echo "FATAL: cannot find $f" >&2; exit 2; }
done

pass=0; fail=0; skip=0
ok()   { printf 'PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad()  { printf 'FAIL  %s%s\n' "$1" "${2:+  — $2}"; fail=$((fail + 1)); }
skp()  { printf 'SKIP  %s%s\n' "$1" "${2:+  — $2}"; skip=$((skip + 1)); }
note() { printf 'NOTE  %s\n' "$1"; }
section() { printf '\n── %s ──\n' "$1"; }
is() {
    local name=$1 want=$2 got=$3
    if [ "$got" = "$want" ]; then ok "$name"
    else bad "$name" "want [$want] got [$got]"; fi
}
finish() {
    printf '\napex-greet-session-bus: %d passed, %d failed, %d skipped\n' \
        "$pass" "$fail" "$skip"
    [ "$fail" -eq 0 ]
}

for c in dbus-daemon dbus-run-session gdbus python3; do
    command -v "$c" >/dev/null 2>&1 || {
        printf 'SKIP: %s is not installed here.\n' "$c"
        printf '      This is a COULD-NOT-RUN, not a pass. Nothing below was measured.\n'
        exit 0
    }
done

W="$(mktemp -d "${TMPDIR:-/tmp}/apex-greet-bus.XXXXXX")" || exit 2
chmod 700 "$W"
cleanup() { rm -rf "$W"; }
trap cleanup EXIT

# ═════════════════════════════════════════════════════════════════════════════
section "what the image really runs at the login screen"
# ═════════════════════════════════════════════════════════════════════════════
# greetd-config.toml holds THREE `command =` lines: the live one, the abandoned
# cage host and the labwc fallback. The other two are commented out, and a grep
# cannot tell which is which — the abandoned line is the one that names `qs`
# directly, so a grep for the greeter's client finds the dead line first. The
# file is therefore parsed as the TOML greetd parses, not read as text. A second
# uncommented `command` would be a duplicate key, which tomllib refuses, so
# "exactly one live command" costs nothing extra here.

COMMAND="$(python3 - "$GREETD_TOML" <<'PY' 2>"$W/toml.err"
import sys, tomllib
with open(sys.argv[1], 'rb') as fh:
    doc = tomllib.load(fh)
print(doc['default_session']['command'])
PY
)"
if [ -n "$COMMAND" ]; then
    ok "greetd-config.toml parses as TOML with exactly one live default_session.command"
    printf '      command = %s\n' "$COMMAND"
else
    bad "greetd-config.toml parses as TOML with exactly one live default_session.command" \
        "$(tr '\n' ' ' <"$W/toml.err")"
fi

# Phrased as "not one of the two dead hosts" rather than "starts with sway" on
# purpose. The fix this suite is waiting for (NEXT item 1) is a wrapper in front
# of the command — `dbus-run-session -- sway …` — and a check anchored on the
# first word would go red for that, beside the assertion that is SUPPOSED to go
# red, and send whoever makes the fix looking for a second problem. That the
# chain really does reach a compositor host and a client is asserted below, by
# running it.
case "$COMMAND" in
    *cage*)
        bad "the live command is not one of the two hosts kept in comments" \
            "it is the ABANDONED cage host — cage 0.2.0 serves quickshell no layer-shell, so the greeter launches and never paints" ;;
    *labwc*)
        bad "the live command is not one of the two hosts kept in comments" \
            "it is the labwc FALLBACK; if that swap is deliberate, update this suite — the labwc autostart section below already covers that host" ;;
    *)  ok "the live command is not one of the two hosts kept in comments" ;;
esac

# The command names an INSTALLED path. Hardcoding the repo file here would
# measure a file the image might not ship — the defect K12/K13 caught in the
# installer, where a launcher was repointed at a path nothing installed and
# every source-reading assertion stayed green. So the installed path is resolved
# back to the repo through Containerfile.base's own COPY table.
copy_src_for() {   # copy_src_for <installed path> -> repo-relative source, or empty
    python3 - "$CF" "$1" <<'PY'
import re, sys
cf, want = sys.argv[1], sys.argv[2]
best = ('', '')
text = open(cf, errors='replace').read()
# Join continuation lines so a COPY split over two lines still resolves.
text = re.sub(r'\\\n\s*', ' ', text)
for line in text.splitlines():
    s = line.strip()
    if not s.upper().startswith('COPY '):
        continue
    parts = s.split()[1:]
    parts = [p for p in parts if not p.startswith('--')]
    if len(parts) < 2:
        continue
    src, dst = parts[-2], parts[-1]
    if want == dst or want.startswith(dst.rstrip('/') + '/'):
        if len(dst) > len(best[1]):
            rest = want[len(dst.rstrip('/')):].lstrip('/')
            best = (src + ('/' + rest if rest else ''), dst)
print(best[0])
PY
}

CONF_PATH="$(printf '%s\n' "$COMMAND" | sed -n 's/.*-c[[:space:]]\{1,\}\([^[:space:]]*\).*/\1/p')"
if [ -n "$CONF_PATH" ]; then
    ok "the sway host config the command names is an absolute installed path ($CONF_PATH)"
else
    bad "the sway host config the command names is an absolute installed path" \
        "no -c argument in [$COMMAND]"
fi

SRC="$(copy_src_for "$CONF_PATH")"
is "the host config it names is the file in this repo" \
   "files/desktop/apex-greet/sway-greet.conf" "$SRC"

GREETD_DST="$(python3 - "$CF" <<'PY'
import re, sys
text = re.sub(r'\\\n\s*', ' ', open(sys.argv[1], errors='replace').read())
for line in text.splitlines():
    s = line.strip()
    if s.upper().startswith('COPY ') and 'greetd-config.toml' in s:
        print(s.split()[-1]); break
PY
)"
is "this greetd config IS the live /etc/greetd/config.toml, not an example" \
   "/etc/greetd/config.toml" "$GREETD_DST"

# ═════════════════════════════════════════════════════════════════════════════
section "the chain, executed: what the greeter's own client can see"
# ═════════════════════════════════════════════════════════════════════════════
# The stubs stand in for everything that would paint or kill. The chain itself —
# greetd's command string, sway's exec line, the shell that joins them — is the
# shipped one, so a wrapper added anywhere in it (dbus-run-session, uwsm, a
# systemd --user run) is really executed and really changes the answer.

STUB="$W/bin"
mkdir -p "$STUB" "$W/home" "$W/run" "$W/no-services"
chmod 700 "$W/run"

cat >"$STUB/sway" <<'EOF'
#!/bin/sh
# stub sway: no compositor, no display. Reads the config it was handed and runs
# its exec lines through sh, which is what sway does with them.
conf=
while [ $# -gt 0 ]; do
    case "$1" in
        -c) conf="$2"; shift 2 ;;
        *)  shift ;;
    esac
done
[ -n "$conf" ] || { echo "stub sway: no -c config" >&2; exit 2; }
printf 'sway-stub-ran\n' >>"$CHAIN_LOG"
sed -n 's/^[[:space:]]*exec\(_always\)\?[[:space:]]\{1,\}"\(.*\)"[[:space:]]*$/\2/p' "$conf" \
    | while IFS= read -r line; do sh -c "$line"; done
EOF

cat >"$STUB/labwc" <<'EOF'
#!/bin/sh
# stub labwc: `labwc -e` (exit the running instance) must SUCCEED so the
# autostart never falls through to its `pkill -TERM -x labwc` branch.
printf 'labwc-stub-ran %s\n' "$*" >>"$CHAIN_LOG"
exit 0
EOF

cat >"$STUB/swaymsg" <<'EOF'
#!/bin/sh
printf 'swaymsg-stub-ran %s\n' "$*" >>"$CHAIN_LOG"
exit 0
EOF

cat >"$STUB/pkill" <<'EOF'
#!/bin/sh
# Never signal anything. The labwc autostart's fallback branch ends in
# `pkill -TERM -x labwc`; a suite that reaches it on a developer's machine would
# kill a real compositor.
printf 'REFUSED pkill %s\n' "$*" >>"$CHAIN_LOG"
exit 0
EOF

# The probe. This is what quickshell would be, reduced to the one question that
# decides whether a screen reader hears anything: can this process reach a
# session bus, and through it the accessibility bus?
cat >"$STUB/qs" <<'EOF'
#!/bin/sh
printf 'qs-stub-ran\n' >>"$CHAIN_LOG"
{
    printf 'ran=1\n'
    printf 'DBUS_SESSION_BUS_ADDRESS=%s\n' "${DBUS_SESSION_BUS_ADDRESS-<unset>}"
    if gdbus call --session --dest org.freedesktop.DBus \
            --object-path /org/freedesktop/DBus \
            --method org.freedesktop.DBus.GetId >/dev/null 2>"$PROBE_OUT.buserr"; then
        printf 'session_bus=yes\n'
    else
        printf 'session_bus=no\n'
        printf 'session_bus_error=%s\n' "$(tr -d '\n' <"$PROBE_OUT.buserr")"
    fi
    if gdbus call --session --dest org.a11y.Bus --object-path /org/a11y/bus \
            --method org.a11y.Bus.GetAddress >"$PROBE_OUT.a11y" 2>&1; then
        printf 'a11y_bus=yes\n'
    else
        printf 'a11y_bus=no\n'
        printf 'a11y_bus_error=%s\n' "$(tr -d '\n' <"$PROBE_OUT.a11y" | tail -c 160)"
    fi
} >"$PROBE_OUT"
EOF
cp "$STUB/qs" "$STUB/quickshell"
chmod +x "$STUB"/*

probe_get() { sed -n "s/^$2=//p" "$1" | head -1; }

# run_chain <probe file> <chain log> <command string> [wrapper words...]
run_chain() {
    local out="$1" log="$2" cmd="$3"; shift 3
    : >"$out"; : >"$log"
    env -i PATH="$STUB:/usr/bin:/bin" HOME="$W/home" USER="${USER:-tester}" \
        XDG_RUNTIME_DIR="$W/run" PROBE_OUT="$out" CHAIN_LOG="$log" \
        "$@" sh -c "$cmd" >>"$log" 2>&1
}

# The command is the shipped one with only the CONFIG PATH redirected at this
# repository's copy — the binary names, the flags and the shell are untouched.
CHAIN_CMD="${COMMAND//$CONF_PATH/$SWAY_CONF}"
if [ "$CHAIN_CMD" = "$COMMAND" ] && [ -n "$CONF_PATH" ]; then
    bad "the chain under test reads THIS repo's sway config" \
        "the config path was not redirected; the run would read the installed file"
else
    ok "the chain under test reads THIS repo's sway config"
fi

run_chain "$W/bare.probe" "$W/bare.log" "$CHAIN_CMD"

# Floor assertion. Everything below is a claim about what the greeter's client
# sees, and all of it is vacuously true if the client never ran. This is the
# assertion that stops "the greeter has no session bus" being reported about a
# chain that fell over on its first line.
if [ "$(probe_get "$W/bare.probe" ran)" = "1" ]; then
    ok "the shipped chain really reaches the greeter's own client"
else
    bad "the shipped chain really reaches the greeter's own client" \
        "quickshell never ran: $(tr '\n' ' ' <"$W/bare.log" | tail -c 200)"
fi
grep -q '^sway-stub-ran$' "$W/bare.log" \
    && ok "greetd's command starts the compositor host, and the host starts the client" \
    || bad "greetd's command starts the compositor host, and the host starts the client" \
           "$(tr '\n' ' ' <"$W/bare.log" | tail -c 200)"

BARE_ADDR="$(probe_get "$W/bare.probe" DBUS_SESSION_BUS_ADDRESS)"
is "the greeter's client is handed no DBUS_SESSION_BUS_ADDRESS" "<unset>" "$BARE_ADDR"

BARE_BUS="$(probe_get "$W/bare.probe" session_bus)"
if [ "$BARE_BUS" = "no" ]; then
    ok "the greeter's client can reach no session bus at all"
    printf '      %s\n' "$(probe_get "$W/bare.probe" session_bus_error)"
else
    bad "the greeter's client can reach no session bus at all" \
        "it reached one — if you have just given the greeter a session bus, this suite and the P2-003 ledger row both need updating, not deleting"
fi

BARE_A11Y="$(probe_get "$W/bare.probe" a11y_bus)"
if [ "$BARE_A11Y" = "no" ]; then
    ok "so org.a11y.Bus cannot be resolved, and Qt's bridge has nothing to publish on"
else
    bad "so org.a11y.Bus cannot be resolved, and Qt's bridge has nothing to publish on" \
        "the accessibility bus WAS reachable from the greeter chain — update this suite and the ledger"
fi

# ═════════════════════════════════════════════════════════════════════════════
section "the probe is capable of the other answer"
# ═════════════════════════════════════════════════════════════════════════════
# Without this the suite is the house defect: four assertions that a bus is
# absent, measured by a probe that has never been shown able to find one. The
# SAME chain, the SAME probe, one wrapper added — and the answer must change.
#
# The private bus is given an EMPTY service directory, so nothing can be
# ACTIVATED onto it. That keeps the next assertion a statement about what the
# greeter chain execs rather than about whether the machine running this suite
# happens to permit at-spi activation.

cat >"$W/session.conf" <<EOF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:tmpdir=$W</listen>
  <servicedir>$W/no-services</servicedir>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
EOF

run_chain "$W/wrapped.probe" "$W/wrapped.log" "$CHAIN_CMD" \
    dbus-run-session --config-file="$W/session.conf" --

WRAP_BUS="$(probe_get "$W/wrapped.probe" session_bus)"
is "the same probe, given a session bus, finds one" "yes" "$WRAP_BUS"

WRAP_A11Y="$(probe_get "$W/wrapped.probe" a11y_bus)"
if [ "$WRAP_A11Y" = "no" ]; then
    ok "a session bus alone is not enough: the greeter chain starts no accessibility bus"
else
    bad "a session bus alone is not enough: the greeter chain starts no accessibility bus" \
        "something in the chain now provides one — update this suite and the ledger"
fi

# Not an assertion: this one is a property of the machine, not of the repository.
# It is printed because it is the reason the fix is not a one-line
# `dbus-run-session`, and the reason tests/lib/atspi.sh execs the launcher.
run_chain "$W/act.probe" "$W/act.log" "$CHAIN_CMD" dbus-run-session --
case "$(probe_get "$W/act.probe" a11y_bus)" in
    yes) note "on THIS machine org.a11y.Bus can be D-Bus activated, so here a session bus would be most of the fix" ;;
    *)   note "on THIS machine org.a11y.Bus cannot be D-Bus activated: $(probe_get "$W/act.probe" a11y_bus_error)"
         note "  so a session bus alone would still leave the greeter unreadable; the launcher has to be execed" ;;
esac

# ═════════════════════════════════════════════════════════════════════════════
section "the documented fallback host has the same hole"
# ═════════════════════════════════════════════════════════════════════════════
# greetd-config.toml offers labwc as the fallback if sway misbehaves on some
# hardware. A fix applied only to the sway host would leave a machine that took
# the fallback with no reader and nothing to say so.

run_chain "$W/labwc.probe" "$W/labwc.log" "sh '$LABWC_AUTOSTART'"

if [ "$(probe_get "$W/labwc.probe" ran)" = "1" ]; then
    ok "the labwc fallback's autostart really reaches the greeter's own client"
else
    bad "the labwc fallback's autostart really reaches the greeter's own client" \
        "$(tr '\n' ' ' <"$W/labwc.log" | tail -c 200)"
fi
is "the labwc fallback's client can reach no session bus either" \
   "no" "$(probe_get "$W/labwc.probe" session_bus)"
grep -q '^REFUSED pkill' "$W/labwc.log" \
    && note "the autostart's pkill fallback was reached and refused by the stub (labwc -e answered first in production)" \
    || ok "the autostart exits the compositor without reaching its pkill fallback"

finish
