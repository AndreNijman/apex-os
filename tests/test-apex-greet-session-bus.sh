#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-greet-session-bus.sh — the login screen has a D-Bus session bus and
#  an accessibility bus, so the markup a screen reader needs actually reaches
#  one (roadmap P2-003, "screen reader … validated").
#
#  ── What this suite used to say, and why it now says the opposite ───────────
#
#  It was written as a statement of a HOLE, with an instruction in its own
#  header: "when somebody closes it, this suite goes RED and names itself …
#  invert the assertion, do not delete it." That is what happened, and this is
#  the inverted suite.
#
#  The hole: greetd's command was `sway -c …` with no session bus anywhere in
#  the chain. Qt's AT-SPI bridge resolves org.a11y.Bus on the SESSION bus like
#  every other desktop application, so the greeter — whose accessibility markup
#  is correct, and which tests/test-apex-greet-atspi.sh proves a screen reader
#  can both read and OPERATE — published zero nodes in production. The markup
#  was right, the bridge worked, and a blind user at the login screen heard
#  nothing.
#
#  The fix is /usr/libexec/apex-greet-session: greetd runs that, it starts a
#  session bus and execs at-spi-bus-launcher and at-spi2-registryd beside it,
#  and then EXECS the compositor. A one-line `dbus-run-session` would not have
#  done: on an SELinux system org.a11y.Bus cannot be D-Bus ACTIVATED at all
#  (EACCES, no AVC, and a byte-identical copy of the same binary under a
#  different label activates perfectly), so the launcher has to be execed.
#
#  ── The half that matters more than the accessibility half ──────────────────
#
#  greetd is boot-critical. This suite exists at least as much to prove that the
#  wrapper CANNOT stop the login screen from starting as to prove that the bus
#  arrives. So the sections below take each piece away in turn — no dbus-daemon,
#  no writable runtime directory — and require the greeter's own client to run
#  anyway; and they require the PID the chain ends on to be the compositor's
#  own, because a middleman that does not forward greetd's teardown signal
#  leaves a compositor holding the VT and the DRM master on a machine nobody can
#  log in to.
#
#  ── What it will not do ─────────────────────────────────────────────────────
#
#  It starts no compositor, opens no window, touches no display and reaches no
#  bus belonging to whoever is sitting at the machine. `sway`, `labwc`, `qs`,
#  `swaymsg` and `pkill` are all stubs on a PATH private to this run — `pkill`
#  most of all, because the labwc fallback's autostart ends in
#  `pkill -TERM -x labwc` and this suite is not permitted to send that signal to
#  anything real. Every run is under `env -i` with a private XDG_RUNTIME_DIR,
#  and every bus the chain starts is killed afterwards by the pid the wrapper
#  recorded — never by pattern, which would reap another suite's private bus.
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
LABWC_RC="$ROOT/files/desktop/apex-greet/labwc-greet/rc.xml"
WRAPPER="$ROOT/files/system/libexec/apex-greet-session"
CF="$ROOT/Containerfile.base"
for f in "$GREETD_TOML" "$SWAY_CONF" "$LABWC_AUTOSTART" "$LABWC_RC" "$WRAPPER" "$CF"; do
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

# Every process the chain leaves behind is killed by the pid the wrapper wrote
# down, never by name: this repository runs several suites at once and a
# `pkill dbus-daemon` would take another one's private bus with it.
reap() {
    local f p
    # Every pid file anywhere under this run's scratch directory, not a fixed
    # list of paths. The first version globbed "$W"/run*/apex-greet-bus and
    # leaked a bus, a launcher and a registry per suite run, for two reasons
    # worth writing down: several chain runs SHARED one runtime directory and so
    # overwrote each other's pid files, and the unwritable-runtime-directory
    # case makes the wrapper fall back to a mktemp directory that was not under
    # $W at all. Each chain run now gets its own runtime directory and the
    # wrapper's fallback is steered here with TMPDIR, so `find` sees all of it.
    while IFS= read -r f; do
        p="$(cat "$f" 2>/dev/null)"
        case "$p" in ''|*[!0-9]*) continue ;; esac
        kill -TERM "$p" 2>/dev/null
    done <<<"$(find "$W" -name '*.pid' -type f 2>/dev/null)"
}
cleanup() { reap; rm -rf "$W"; }
trap cleanup EXIT INT TERM

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

case "$COMMAND" in
    *cage*)
        bad "the live command is not one of the two hosts kept in comments" \
            "it is the ABANDONED cage host — cage 0.2.0 serves quickshell no layer-shell, so the greeter launches and never paints" ;;
    *labwc*)
        bad "the live command is not one of the two hosts kept in comments" \
            "it is the labwc FALLBACK; if that swap is deliberate, update this suite — the labwc autostart section below already covers that host" ;;
    *)  ok "the live command is not one of the two hosts kept in comments" ;;
esac

# The command names INSTALLED paths. Hardcoding the repo files here would
# measure something the image might not ship — the K12/K13 defect, where a
# launcher was repointed at a path nothing installed and every source-reading
# assertion stayed green while the ISO would have booted with no installer at
# all. So each installed path is resolved back to the repo through
# Containerfile.base's own COPY table.
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

# ── the wrapper ─────────────────────────────────────────────────────────────
# The whole fix is that greetd runs this instead of running sway directly. It is
# the first word of the command, and it is an installed path, so it gets exactly
# the treatment the host config gets: resolved back through the COPY table
# rather than assumed to be the file sitting in this checkout.
WRAP_PATH="${COMMAND%% *}"
case "$WRAP_PATH" in
    /*) ok "greetd's command begins with an absolute installed path ($WRAP_PATH)" ;;
    *)  bad "greetd's command begins with an absolute installed path" "got [$WRAP_PATH]" ;;
esac

WRAP_SRC="$(copy_src_for "$WRAP_PATH")"
is "and that path is the session wrapper in this repo" \
   "files/system/libexec/apex-greet-session" "$WRAP_SRC"

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

# The last line of the wrapper is the only unconditional statement in it. It is
# asserted here as well as in Containerfile.base because a wrapper that forgot
# to exec is a login screen that never paints, and there is no failure mode in
# this repository more expensive than that one.
if [ "$(tail -1 "$WRAPPER")" = 'exec "$@"' ]; then
    ok "the wrapper's last act is to exec what it was handed"
else
    bad "the wrapper's last act is to exec what it was handed" \
        "last line is [$(tail -1 "$WRAPPER")] — greetd would hold a shell, not the compositor"
fi

# ═════════════════════════════════════════════════════════════════════════════
section "the host config still parses, and the parse check can fail"
# ═════════════════════════════════════════════════════════════════════════════
# sway-greet.conf gained a binding — the one key a user who cannot see the
# screen can press to start a reader. A typo in it is a greeter that refuses its
# config, so it is put through sway's own parser. The negative control is the
# point: a check that always says "valid" is not a check, and `sway -C` prints
# its errors and could easily have exited 0 for both.
sway -C -c "$SWAY_CONF" >"$W/sway.out" 2>&1; SWAY_RC=$?
if ! command -v sway >/dev/null 2>&1; then
    skp "the greeter's sway config parses" "sway is not installed here; COULD-NOT-RUN"
elif [ "$SWAY_RC" -eq 126 ]; then
    # /usr/bin/sway carries the file capability cap_sys_nice=ep, and exec of a
    # binary with an EFFECTIVE file capability outside the bounding set is
    # EPERM. Inside a container sway therefore cannot run at all, for a good
    # config and a broken one alike — which is exactly why this check is here
    # and not in Containerfile.base.
    skp "the greeter's sway config parses" \
        "sway cannot be executed here (rc=126; cap_sys_nice outside the bounding set); COULD-NOT-RUN"
    skp "…and sway -C really does refuse a config with a bad binding" "same reason"
else
    if [ "$SWAY_RC" -eq 0 ]; then
        ok "the greeter's sway config parses"
    else
        bad "the greeter's sway config parses" "$(grep ERROR "$W/sway.out" | head -2 | tr '\n' ' ')"
    fi
    sed 's/^bindsym /bindsym --no-such-flag /' "$SWAY_CONF" >"$W/broken.conf"
    sway -C -c "$W/broken.conf" >/dev/null 2>&1
    if [ $? -ne 0 ]; then
        ok "…and sway -C really does refuse a config with a bad binding"
    else
        bad "…and sway -C really does refuse a config with a bad binding" \
            "it accepted a deliberately broken one, so the check above proves nothing"
    fi
fi

# The one key. Asserted on both hosts, because a machine that took the labwc
# fallback would otherwise have a login screen with no way to ask for a reader
# and nothing to say so.
grep -q 'apex-screen-reader' "$SWAY_CONF" \
    && ok "the sway host binds a key that starts the screen reader" \
    || bad "the sway host binds a key that starts the screen reader" \
           "a user who cannot see the screen has no way to ask for one"
grep -q 'apex-screen-reader' "$LABWC_RC" \
    && ok "the labwc fallback host binds the same key" \
    || bad "the labwc fallback host binds the same key" \
           "the fallback would leave a blind user with no reader and no way to know"
# labwc has no `-C`-style validate verb, so the config gets the check its format
# allows. A malformed rc.xml is a fallback host that comes up with labwc's
# BUILT-IN keybindings instead of this file's — which is a greeter a stray
# shortcut can escape from.
if python3 -c "import xml.etree.ElementTree as ET,sys; ET.parse(sys.argv[1])" "$LABWC_RC" 2>"$W/rc.err"; then
    ok "and the fallback host's rc.xml is well-formed XML"
else
    bad "and the fallback host's rc.xml is well-formed XML" \
        "$(tr '\n' ' ' <"$W/rc.err" | tail -c 120)"
fi

# ═════════════════════════════════════════════════════════════════════════════
section "the chain, executed: what the greeter's own client can see"
# ═════════════════════════════════════════════════════════════════════════════
# The stubs stand in for everything that would paint or kill. The chain itself —
# greetd's command string, the wrapper, sway's exec line, the shell that joins
# them — is the shipped one, so what the wrapper exports is really exported and
# really changes the answer.

STUB="$W/bin"
mkdir -p "$STUB" "$W/home" "$W/no-services"

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
# The pid this process ended up with. greetd holds the pid of what it started
# and signals THAT on a successful login; if the wrapper had forked instead of
# exec'ing, this number would not be the one greetd is holding.
printf 'sway-stub-pid %s\n' "$$" >>"$CHAIN_LOG"
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
# `pkill -TERM -x labwc`; a suite that reached it on a developer's machine would
# kill a real compositor.
printf 'REFUSED pkill %s\n' "$*" >>"$CHAIN_LOG"
exit 0
EOF

# The probe. This is what quickshell would be, reduced to the questions that
# decide whether a screen reader hears anything: can this process reach a
# session bus, through it the accessibility bus, and is there a registry on that
# bus for a reader to enumerate the tree with?
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
    # The a11y bus is started in the background by the wrapper and the greeter's
    # client is the thing that has to find it, so the question is asked the way
    # an application asks it -- with a bounded wait, because "not yet" and
    # "never" are different answers and only one of them is a defect.
    a11y=no; addr=
    i=0
    while [ "$i" -lt 40 ]; do
        if addr="$(gdbus call --session --dest org.a11y.Bus --object-path /org/a11y/bus \
                        --method org.a11y.Bus.GetAddress 2>"$PROBE_OUT.a11yerr")"; then
            a11y=yes; break
        fi
        i=$((i + 1)); sleep 0.25
    done
    printf 'a11y_bus=%s\n' "$a11y"
    if [ "$a11y" = no ]; then
        printf 'a11y_bus_error=%s\n' "$(tr -d '\n' <"$PROBE_OUT.a11yerr" | tail -c 160)"
    else
        addr="$(printf '%s' "$addr" | sed -e "s/^('//" -e "s/',)$//")"
        printf 'a11y_addr=%s\n' "$addr"
        reg=no
        i=0
        while [ "$i" -lt 40 ]; do
            if gdbus call --address "$addr" --dest org.freedesktop.DBus \
                    --object-path /org/freedesktop/DBus \
                    --method org.freedesktop.DBus.ListNames 2>/dev/null \
                    | grep -q 'org.a11y.atspi.Registry'; then
                reg=yes; break
            fi
            i=$((i + 1)); sleep 0.25
        done
        printf 'registry=%s\n' "$reg"
    fi
} >"$PROBE_OUT"
EOF
cp "$STUB/qs" "$STUB/quickshell"
chmod +x "$STUB"/*

probe_get() { sed -n "s/^$2=//p" "$1" | head -1; }

CHAIN_PID=""
CHAIN_N=0
# run_chain <probe file> <chain log> <command string> [wrapper words...]
# Runs in the background and waits, so the pid the chain STARTS on is knowable
# here — that is what the exec assertion compares against.
#
# A FRESH runtime directory per run, because the wrapper records what it started
# in pid files under it and two runs sharing one directory overwrite each
# other's — which is a leaked bus, launcher and registry per suite run, on a
# machine where several suites run at once. TMPDIR points into $W as well, so
# even the wrapper's own mktemp fallback lands somewhere cleanup can find.
run_chain() {
    local out="$1" log="$2" cmd="$3" rt; shift 3
    CHAIN_N=$((CHAIN_N + 1))
    rt="${CHAIN_RUNTIME:-$W/run-$CHAIN_N}"
    mkdir -p "$rt" 2>/dev/null; chmod 700 "$rt" 2>/dev/null
    : >"$out"; : >"$log"
    env -i PATH="$STUB:/usr/bin:/bin" HOME="$W/home" USER="${USER:-tester}" \
        TMPDIR="$W" XDG_RUNTIME_DIR="$rt" PROBE_OUT="$out" CHAIN_LOG="$log" \
        "$@" sh -c "$cmd" >>"$log" 2>&1 &
    CHAIN_PID=$!
    wait "$CHAIN_PID"
}

# The command is the shipped one with only the INSTALLED PATHS redirected at
# this repository's copies — the flags, the argument order and the shell are
# untouched, so the wrapper under test is really the wrapper the image runs.
CHAIN_CMD="${COMMAND//$CONF_PATH/$SWAY_CONF}"
CHAIN_CMD="${CHAIN_CMD//$WRAP_PATH/$WRAPPER}"
if [ "$CHAIN_CMD" = "$COMMAND" ]; then
    bad "the chain under test reads THIS repo's wrapper and sway config" \
        "no path was redirected; the run would read the installed files"
else
    ok "the chain under test reads THIS repo's wrapper and sway config"
fi

run_chain "$W/bare.probe" "$W/bare.log" "$CHAIN_CMD"

# Floor assertion. Everything below is a claim about what the greeter's client
# sees, and all of it is vacuously true if the client never ran.
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

# ── the assertion that is really about not breaking the login ───────────────
# greetd terminates the greeter by signalling the pid it started. If the wrapper
# forked instead of exec'ing, that pid belongs to a shell and the compositor
# survives it, holding the VT and the DRM master — a machine that cannot be
# logged into. This is the reason the fix is not `dbus-run-session --` in front
# of the command.
SWAY_PID="$(sed -n 's/^sway-stub-pid //p' "$W/bare.log" | head -1)"
if [ -n "$SWAY_PID" ] && [ "$SWAY_PID" = "$CHAIN_PID" ]; then
    ok "the pid greetd would hold is the compositor's own — the wrapper execs, it does not fork"
else
    bad "the pid greetd would hold is the compositor's own" \
        "chain started as $CHAIN_PID, compositor ran as ${SWAY_PID:-<none>}; greetd's teardown would miss it"
fi

BARE_ADDR="$(probe_get "$W/bare.probe" DBUS_SESSION_BUS_ADDRESS)"
if [ "$BARE_ADDR" != "<unset>" ] && [ -n "$BARE_ADDR" ]; then
    ok "the greeter's client is handed a DBUS_SESSION_BUS_ADDRESS"
else
    bad "the greeter's client is handed a DBUS_SESSION_BUS_ADDRESS" \
        "it is $BARE_ADDR — Qt's accessibility bridge has nothing to resolve org.a11y.Bus on"
fi
case "$BARE_ADDR" in
    *"$W/"*)
        ok "and the bus it names is inside this run's own runtime directory" ;;
    *)  bad "and the bus it names is inside this run's own runtime directory" \
            "[$BARE_ADDR] — a greeter bus must not be somewhere another user can join" ;;
esac

BARE_BUS="$(probe_get "$W/bare.probe" session_bus)"
if [ "$BARE_BUS" = "yes" ]; then
    ok "the greeter's client can reach a session bus"
else
    bad "the greeter's client can reach a session bus" \
        "$(probe_get "$W/bare.probe" session_bus_error)"
fi

# The accessibility bus needs at-spi2-core on the machine running this. A runner
# without it is a COULD-NOT-RUN with a name, never a pass: "no bus found" is
# exactly what the defect looked like.
A11Y_LAUNCHER=""
for c in /usr/libexec/at-spi-bus-launcher /usr/lib/at-spi2-core/at-spi-bus-launcher \
         /usr/libexec/at-spi2-core/at-spi-bus-launcher; do
    [ -x "$c" ] && { A11Y_LAUNCHER="$c"; break; }
done
BARE_A11Y="$(probe_get "$W/bare.probe" a11y_bus)"
if [ -z "$A11Y_LAUNCHER" ]; then
    skp "org.a11y.Bus resolves from inside the greeter chain" \
        "at-spi-bus-launcher is not installed here, so the wrapper had nothing to exec; COULD-NOT-RUN"
    skp "and a screen reader has a registry to enumerate the tree with" \
        "same reason"
elif [ "$BARE_A11Y" = "yes" ]; then
    ok "org.a11y.Bus resolves from inside the greeter chain ($(probe_get "$W/bare.probe" a11y_addr))"
    if [ "$(probe_get "$W/bare.probe" registry)" = "yes" ]; then
        ok "and a screen reader has a registry to enumerate the tree with"
    else
        bad "and a screen reader has a registry to enumerate the tree with" \
            "org.a11y.atspi.Registry never took its name — the bus answers and a reader still sees nothing, which is the most deceptive failure available here"
    fi
else
    bad "org.a11y.Bus resolves from inside the greeter chain" \
        "$(probe_get "$W/bare.probe" a11y_bus_error)"
    skp "and a screen reader has a registry to enumerate the tree with" \
        "no accessibility bus to look on"
fi

# ── and it takes its buses away again ──────────────────────────────────────
# The wrapper execs, so it is not around to clean up; the watcher it leaves
# behind is. Without one, every login and every greeter restart would leave a
# session bus, an accessibility bus, a launcher and a registry behind as orphans
# owned by the greetd user — and whether greetd's own teardown reaps them is a
# property of greetd that this repository has not measured, which is why the
# cleanup is owned here rather than assumed there.
#
# Checked BEFORE this suite's own reap runs, or it would be measuring itself.
bare_busdir="$W/run-1/apex-greet-bus"
still_alive() {
    local f p n=0
    for f in bus.pid a11y.pid registry.pid; do
        p="$(cat "$bare_busdir/$f" 2>/dev/null)"
        case "$p" in ''|*[!0-9]*) continue ;; esac
        kill -0 "$p" 2>/dev/null && n=$((n + 1))
    done
    printf '%s' "$n"
}
if [ ! -d "$bare_busdir" ]; then
    skp "the wrapper takes its buses away when the compositor goes" \
        "no bus directory was made, so there is nothing to have cleaned up"
else
    for _ in $(seq 1 30); do [ "$(still_alive)" = "0" ] && break; sleep 0.5; done
    if [ "$(still_alive)" = "0" ]; then
        ok "the wrapper takes its buses away when the compositor goes"
    else
        bad "the wrapper takes its buses away when the compositor goes" \
            "$(still_alive) of its processes are still running 15s after the chain exited — every login would leave a set behind"
    fi
fi

# ═════════════════════════════════════════════════════════════════════════════
section "the probe is capable of the other answer"
# ═════════════════════════════════════════════════════════════════════════════
# Without this the section above is four assertions that a bus is present,
# measured by a probe that has never been shown able to fail. The SAME chain
# with the wrapper REMOVED is the exact state the greeter shipped in until this
# round, and it must produce the old answer.

BARE_CMD="${CHAIN_CMD#"$WRAPPER" }"
if [ "$BARE_CMD" = "$CHAIN_CMD" ]; then
    bad "the wrapper can be taken out of the chain for the control below" \
        "the command does not begin with the wrapper, so the control is not a control"
else
    ok "the wrapper can be taken out of the chain for the control below"
fi
run_chain "$W/nowrap.probe" "$W/nowrap.log" "$BARE_CMD"
is "with the wrapper gone, the client is handed no bus address" \
   "<unset>" "$(probe_get "$W/nowrap.probe" DBUS_SESSION_BUS_ADDRESS)"
is "with the wrapper gone, the client can reach no session bus at all" \
   "no" "$(probe_get "$W/nowrap.probe" session_bus)"
is "with the wrapper gone, org.a11y.Bus cannot be resolved" \
   "no" "$(probe_get "$W/nowrap.probe" a11y_bus)"

# ═════════════════════════════════════════════════════════════════════════════
section "every piece of the wrapper is optional, and the login is not"
# ═════════════════════════════════════════════════════════════════════════════
# greetd is boot-critical. This is the half of the suite that is not about
# accessibility at all: whatever is missing, whatever fails, the compositor must
# still start. Each case takes one piece away and requires the greeter's own
# client to run regardless.

NOBUS="$W/nobus-bin"
mkdir -p "$NOBUS"
# Everything the chain needs, one by one, so that "dbus-daemon is missing" is
# the ONLY thing missing. An incomplete list here produces a failure that reads
# exactly like the defect this case exists to rule out — the first draft left
# out `sh` and reported that the wrapper had stopped the login screen.
for b in sh bash env sed tr grep sleep cat mkdir chmod mktemp printf head tail \
         cut basename dirname rm ln id seq sort test expr \
         sway labwc swaymsg pkill qs quickshell gdbus; do
    [ -e "$STUB/$b" ] && { cp "$STUB/$b" "$NOBUS/$b"; continue; }
    p="$(command -v "$b" 2>/dev/null)" && ln -sf "$p" "$NOBUS/$b"
done
# A PATH with everything the chain needs EXCEPT dbus-daemon.
run_chain_nodbus() {
    : >"$1"; : >"$2"
    env -i PATH="$NOBUS" HOME="$W/home" USER="${USER:-tester}" \
        TMPDIR="$W" XDG_RUNTIME_DIR="$W/run-nodbus" PROBE_OUT="$1" CHAIN_LOG="$2" \
        sh -c "$3" >>"$2" 2>&1
}
mkdir -p "$W/run-nodbus"; chmod 700 "$W/run-nodbus"
run_chain_nodbus "$W/nodbus.probe" "$W/nodbus.log" "$CHAIN_CMD"
if [ "$(probe_get "$W/nodbus.probe" ran)" = "1" ]; then
    ok "with no dbus-daemon on the machine, the greeter still starts"
else
    bad "with no dbus-daemon on the machine, the greeter still starts" \
        "the wrapper stopped the login screen over a missing optional dependency: $(tr '\n' ' ' <"$W/nodbus.log" | tail -c 200)"
fi
is "…and honestly reports that there is no bus, rather than pretending" \
   "no" "$(probe_get "$W/nodbus.probe" session_bus)"

# An unwritable runtime directory. The wrapper falls back to a private mktemp
# dir; what it must never do is fail.
mkdir -p "$W/run-ro" && chmod 500 "$W/run-ro"
CHAIN_RUNTIME="$W/run-ro" run_chain "$W/ro.probe" "$W/ro.log" "$CHAIN_CMD"
if [ "$(probe_get "$W/ro.probe" ran)" = "1" ]; then
    ok "with an unwritable runtime directory, the greeter still starts"
else
    bad "with an unwritable runtime directory, the greeter still starts" \
        "$(tr '\n' ' ' <"$W/ro.log" | tail -c 200)"
fi
chmod 700 "$W/run-ro" 2>/dev/null

# No runtime directory at all — the shape a greetd session takes if logind has
# not made one. The wrapper falls back to a private 0700 mktemp directory; what
# it must do is still start the compositor, and what it should do is still get
# the greeter a bus.
: >"$W/nort.probe"; : >"$W/nort.log"
env -i PATH="$STUB:/usr/bin:/bin" HOME="$W/home" USER="${USER:-tester}" \
    TMPDIR="$W" PROBE_OUT="$W/nort.probe" CHAIN_LOG="$W/nort.log" \
    sh -c "$CHAIN_CMD" >>"$W/nort.log" 2>&1
if [ "$(probe_get "$W/nort.probe" ran)" = "1" ]; then
    ok "with no runtime directory at all, the greeter still starts"
else
    bad "with no runtime directory at all, the greeter still starts" \
        "$(tr '\n' ' ' <"$W/nort.log" | tail -c 200)"
fi
is "…and the wrapper's private fallback still gets it a bus" \
   "yes" "$(probe_get "$W/nort.probe" session_bus)"

# The wrapper with no arguments at all. Not a login path — greetd always passes
# a command — but a wrapper that silently execs nothing would be a black screen
# with no message, and this is the one case where failing loudly is right.
env -i PATH="$STUB:/usr/bin:/bin" "$WRAPPER" >"$W/noargs.out" 2>&1
if [ $? -ne 0 ] && grep -q 'no compositor command' "$W/noargs.out"; then
    ok "asked to run nothing, the wrapper says so and exits non-zero"
else
    bad "asked to run nothing, the wrapper says so and exits non-zero" \
        "rc=$? out=[$(tr '\n' ' ' <"$W/noargs.out")]"
fi

# ═════════════════════════════════════════════════════════════════════════════
section "the documented fallback host gets the same treatment"
# ═════════════════════════════════════════════════════════════════════════════
# greetd-config.toml offers labwc as the fallback if sway misbehaves on some
# hardware. A fix applied only to the sway host would leave a machine that took
# the fallback with a login screen no reader can hear, and nothing to say so.

if grep -q '^#[[:space:]]*command = "/usr/libexec/apex-greet-session labwc' "$GREETD_TOML"; then
    ok "the documented labwc fallback command carries the wrapper too"
else
    bad "the documented labwc fallback command carries the wrapper too" \
        "the swap-in line in greetd-config.toml would start labwc with no session bus"
fi

# The wrapper in front of the fallback's own startup path, which is what that
# command amounts to once labwc has read its config directory.
run_chain "$W/labwc.probe" "$W/labwc.log" "'$WRAPPER' sh '$LABWC_AUTOSTART'"
if [ "$(probe_get "$W/labwc.probe" ran)" = "1" ]; then
    ok "the labwc fallback's autostart really reaches the greeter's own client"
else
    bad "the labwc fallback's autostart really reaches the greeter's own client" \
        "$(tr '\n' ' ' <"$W/labwc.log" | tail -c 200)"
fi
is "and that client can reach a session bus as well" \
   "yes" "$(probe_get "$W/labwc.probe" session_bus)"
grep -q '^REFUSED pkill' "$W/labwc.log" \
    && note "the autostart's pkill fallback was reached and refused by the stub (labwc -e answered first in production)" \
    || ok "the autostart exits the compositor without reaching its pkill fallback"

# Not an assertion: a property of the machine rather than of the repository, and
# the reason the fix execs the launcher instead of relying on activation.
run_chain "$W/act.probe" "$W/act.log" "$BARE_CMD" dbus-run-session --
case "$(probe_get "$W/act.probe" a11y_bus)" in
    yes) note "on THIS machine org.a11y.Bus can also be D-Bus activated, so here a bare session bus would have been most of the fix" ;;
    *)   note "on THIS machine org.a11y.Bus cannot be D-Bus activated: $(probe_get "$W/act.probe" a11y_bus_error)"
         note "  which is why the wrapper execs at-spi-bus-launcher rather than leaving it to activation" ;;
esac

finish
