#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-session-watchdog.sh — the greeter notices that the desktop is not
#  starting, and the notice cannot lock anybody out.
#
#  ── What is under test ──────────────────────────────────────────────────────
#
#  Roadmap P2-018 criterion 1: "a graphics/compositor/shell failure CAN ENTER a
#  conservative recovery desktop". APEX Safe Graphics shipped as a session a
#  PERSON enters. This suite covers the verb: /usr/libexec/apex-session-watchdog
#  counts bounces, and the greeter preselects the recovery session on the third.
#
#  ── The property that matters more than the feature ─────────────────────────
#
#  A counter wired into the login screen is a lockout risk, and the greeter
#  calls it on the only path that reaches Greetd.launch(). So the mutations here
#  are not decoration — §2 replaces the helper with `sleep 60`, then deletes it,
#  then takes `timeout` off PATH, and each time REQUIRES the greeter's own
#  launch script to finish and to have written last-user and last-session. If
#  any of those three goes red, the change under test can strand somebody at a
#  password prompt.
#
#  ── Why it lifts the greeter's shell out of the QML ─────────────────────────
#
#  The same reason tests/test-apex-greet-sessions.sh does, and the same
#  extractor shape: a test that retypes the greeter's `sh -c` body proves only
#  that the test author and the greeter author agreed on the day it was written.
#  Both hooks (`record` on the way out, `check` on the way in) are pulled out of
#  the shipped GreetContext.qml and RUN, with only the state directory and the
#  helper's path repointed. An extraction that returned an empty string would
#  make every assertion below pass for no reason, so a short or shapeless
#  extraction is a FAILURE, never a skip.
#
#  ── What it will not do ─────────────────────────────────────────────────────
#
#  Nothing here starts a compositor, a greeter, a session or greetd; it opens no
#  window, touches no real /var/lib/apex-greet and asks for no password.
#  Everything happens in a temp directory.
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
# Deliberately +e, like the other suites in this directory: CI invokes a suite
# as `bash -e {0}`, and under -e an assignment from a failing command ends the
# run silently, mid-section.
set +e

cd "$(dirname "$0")" || exit 2
ROOT="$(cd .. && pwd)"
WD="$ROOT/files/system/libexec/apex-session-watchdog"
GREETER="$ROOT/files/desktop/apex-greet/GreetContext.qml"
SURFACE="$ROOT/files/desktop/apex-greet/GreetSurface.qml"
SESSIONS_SRC="$ROOT/files/desktop/wayland-sessions"
for f in "$WD" "$GREETER" "$SURFACE"; do
    [ -f "$f" ] || { echo "FATAL: cannot find $f" >&2; exit 2; }
done

pass=0; fail=0; skip=0
ok()  { printf 'PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL  %s%s\n' "$1" "${2:+  — $2}"; fail=$((fail + 1)); }
skp() { printf 'SKIP  %s%s\n' "$1" "${2:+  — $2}"; skip=$((skip + 1)); }
section() { printf '\n── %s ──\n' "$1"; }
is() {
    local name=$1 want=$2 got=$3
    if [ "$got" = "$want" ]; then ok "$name"
    else bad "$name" "want [$want] got [$got]"; fi
}
has() {
    local name=$1 needle=$2 hay=$3
    case "$hay" in *"$needle"*) ok "$name" ;; *) bad "$name" "no [$needle] in [$hay]" ;; esac
}
# Same test, for haystacks that are whole source files — printing thirty
# kilobytes of QML into a CI log on a failure helps nobody. `case` rather than
# `grep -q`, deliberately: this suite runs under `pipefail`, and `printf | grep
# -q` makes printf die of SIGPIPE the moment grep finds its match, so the
# PIPELINE reports 141 and a passing assertion reads as a failure.
hasf() {
    local name=$1 needle=$2 hay=$3
    case "$hay" in *"$needle"*) ok "$name" ;; *) bad "$name" "the file does not contain [$needle]" ;; esac
}

WORK="$(mktemp -d "${TMPDIR:-/tmp}/apex-session-watchdog.XXXXXX")" || exit 2
cleanup() { chmod -R u+rwX "$WORK" 2>/dev/null; rm -rf "$WORK"; }
trap cleanup EXIT INT TERM

# The recovery session's id, as this suite believes it. §4 proves the shipped
# script and the shipped greeter both agree with this one string rather than
# with each other only.
RECOVERY_ID="apex-safe-graphics"

# A sessions directory with the shape of a built image, so the helper's
# "is the recovery entry actually installed" gate has something real to read.
STAGE="$WORK/wayland-sessions"
mkdir -p "$STAGE"
for f in "$SESSIONS_SRC"/*.desktop; do
    [ -f "$f" ] && cp "$f" "$STAGE/"
done

STATE="$WORK/state"
mkdir -p "$STATE"

# Every invocation in §1 goes through this: the real shipped script, pointed at
# the staging tree and nothing else.
wd() { env APEX_SESSION_WATCHDOG_STATE="$STATE" \
           APEX_SESSION_WATCHDOG_SESSIONS="$STAGE" \
           sh "$WD" "$@"; }
# Same, with the tunables moved so a test never has to wait 45 seconds.
wd_env() { local g=$1 t=$2; shift 2
    env APEX_SESSION_WATCHDOG_STATE="$STATE" \
        APEX_SESSION_WATCHDOG_SESSIONS="$STAGE" \
        APEX_SESSION_WATCHDOG_GRACE="$g" \
        APEX_SESSION_WATCHDOG_THRESHOLD="$t" \
        sh "$WD" "$@"; }

# Backdate the pending launch record so "this session survived" can be tested
# without the suite sleeping through the grace period. It edits only the epoch
# field, leaving the helper's own parser to do the rest.
backdate() {
    local secs=$1 line now
    line="$(cat "$STATE/session-launch" 2>/dev/null)" || return 1
    now="$(date +%s)"
    printf '%s %s\n' "$((now - secs))" "${line#* }" > "$STATE/session-launch"
}

reset_state() { rm -f "$STATE/session-launch" "$STATE/session-failures"; }

# ─────────────────────────────────────────────────────────────────────────────
section "§1 the counter, in isolation"
# ─────────────────────────────────────────────────────────────────────────────

if [ -x "$WD" ]; then
    ok "the shipped helper is executable"
else
    bad "the shipped helper is executable" "mode $(stat -c %a "$WD" 2>/dev/null)"
fi

reset_state
out1="$(wd record apex-labwc; wd check)"
is "one bounce selects nothing" "" "$out1"
out2="$(wd record apex-labwc; wd check)"
is "two bounces select nothing" "" "$out2"
out3="$(wd record apex-labwc; wd check)"
is "the THIRD bounce preselects the recovery session" "$RECOVERY_ID" "$out3"

# The one line, and only the one line. A greeter reading a SplitParser would act
# on any line it saw; the helper is not allowed to give it a second one.
lines="$(wd record apex-labwc; wd check | wc -l)"
is "…and check writes exactly one line to stdout" "1" "$lines"

reset_state
wd record apex-labwc >/dev/null
c1="$(wd check; wd check; wd check; wd status | sed -n 's/^failures *: \([0-9]*\).*/\1/p')"
is "the launch record is CONSUMED — one record cannot be counted three times" "1" "$c1"

# greetd re-runs its default_session whenever the greeter exits, including when
# the greeter crashes before any launch. Without consume-on-read that respawn
# alone would walk the counter to the threshold on a healthy machine.

reset_state
for _ in 1 2 3; do wd record apex-labwc >/dev/null; wd check >/dev/null; done
out4="$(wd check)"
is "the count outlives a reboot — a bare check still preselects" "$RECOVERY_ID" "$out4"

reset_state
for _ in 1 2 3; do wd record apex-labwc >/dev/null; wd check >/dev/null; done
wd record apex-labwc >/dev/null
backdate 3600
out5="$(wd check)"
is "a session that SURVIVES the grace period stops the preselection" "" "$out5"
has "…and the counter is cleared, not merely ignored" "failures        : none recorded" "$(wd status)"

reset_state
wd record apex-labwc >/dev/null; wd check >/dev/null
wd record niri >/dev/null;       wd check >/dev/null
has "a different session's bounce restarts the count at one" \
    "failures        : 1 in a row for niri" "$(wd status)"

reset_state
for _ in 1 2 3 4 5; do wd record "$RECOVERY_ID" >/dev/null; out6="$(wd check)"; done
is "recovery bouncing never escalates — there is nowhere to escalate to" "" "$out6"
has "…and recovery's own bounces are not counted at all" \
    "failures        : none recorded" "$(wd status)"

reset_state
for _ in 1 2 3; do wd record apex-labwc >/dev/null; wd check >/dev/null; done
wd record "$RECOVERY_ID" >/dev/null
backdate 3600
out7="$(wd check)"
is "a working recovery session does NOT clear the desktop's failure count" \
    "$RECOVERY_ID" "$out7"

reset_state
for _ in 1 2 3; do wd record apex-labwc >/dev/null; wd check >/dev/null; done
printf '%s apex-labwc\n' "$(( $(date +%s) + 86400 ))" > "$STATE/session-launch"
wd check >/dev/null
has "a clock that went backwards is not evidence of a failure" \
    "failures        : 3 in a row for apex-labwc" "$(wd status)"

reset_state
printf 'not-an-epoch\n' > "$STATE/session-launch"
out8="$(wd check)"; rc8=$?
is "a malformed launch record prints nothing" "" "$out8"
is "…and exits 0" "0" "$rc8"
has "…and counts nothing" "failures        : none recorded" "$(wd status)"

reset_state
out9="$(wd record 'apex-labwc; rm -rf /'; wd status)"
has "an id that is not a plain desktop-entry id is refused" "pending launch  : none" "$out9"
outA="$(wd record ''; wd status)"
has "…and so is an empty one (the greeter's no-sessions login-shell path)" \
    "pending launch  : none" "$outA"

# The recovery entry not being installed is the case where a printed id would
# make the greeter show a reason for a session that is not in its picker.
reset_state
EMPTY_SESSIONS="$WORK/no-sessions"; mkdir -p "$EMPTY_SESSIONS"
for _ in 1 2 3; do wd record apex-labwc >/dev/null; wd check >/dev/null; done
outB="$(env APEX_SESSION_WATCHDOG_STATE="$STATE" \
            APEX_SESSION_WATCHDOG_SESSIONS="$EMPTY_SESSIONS" sh "$WD" check)"
is "with no recovery entry installed, check prints nothing even at threshold" "" "$outB"

# A state directory the greeter cannot create is what it meets if tmpfiles did
# not run. The obvious way to write this — mkdir a directory and chmod it 0555 —
# TESTS NOTHING ON THE ONE MACHINE THAT RUNS THIS SUITE AUTOMATICALLY: CI runs
# as root, root has CAP_DAC_OVERRIDE, and the first version of this section
# therefore turned itself into a SKIP there while passing locally. A skip is a
# could-not-run, and the four assertions that matter most — the ones that say
# the helper is silent when it cannot write — were the ones not running.
#
# So the directory is made uncreatable by SHAPE rather than by permission: the
# parent is a regular file, and mkdir(2) returns ENOTDIR to uid 0 exactly as it
# does to everybody else. Verified under `unshare --user --map-root-user`, which
# is CI's uid without CI.
NODIR="$WORK/not-a-directory"; : > "$NODIR"
outC="$(env APEX_SESSION_WATCHDOG_STATE="$NODIR/state" \
            APEX_SESSION_WATCHDOG_SESSIONS="$STAGE" sh "$WD" record apex-labwc 2>&1)"; rcC=$?
is "a state directory that cannot be created makes record silent" "" "$outC"
is "…and exit 0" "0" "$rcC"
outD="$(env APEX_SESSION_WATCHDOG_STATE="$NODIR/state" \
            APEX_SESSION_WATCHDOG_SESSIONS="$STAGE" sh "$WD" check 2>/dev/null)"; rcD=$?
is "…and check silent" "" "$outD"
is "…and exit 0" "0" "$rcD"

reset_state
for _ in 1 2 3; do wd record apex-labwc >/dev/null; wd check >/dev/null; done
wd reset >/dev/null 2>&1
is "reset forgets the failures" "" "$(wd check)"

outE="$(wd definitely-not-a-verb 2>/dev/null)"; rcE=$?
is "an unknown verb writes NOTHING to stdout" "" "$outE"
is "…and exits 2" "2" "$rcE"

# The tunables are what make the two hook sections below runnable in a second
# rather than in a minute, so they have to actually take effect.
reset_state
wd_env 45 1 record apex-labwc >/dev/null
is "the threshold override is honoured" "$RECOVERY_ID" "$(wd_env 45 1 check)"
reset_state
wd record apex-labwc >/dev/null
backdate 10
is "the grace override is honoured" "" "$(wd_env 5 1 check)"

# ─────────────────────────────────────────────────────────────────────────────
section "§2 the greeter's launch hook, lifted out of the shipped QML"
# ─────────────────────────────────────────────────────────────────────────────
# GreetContext.qml holds both hooks as JS concatenations of string literals
# inside a Process command array. The extractor is the one
# tests/test-apex-greet-sessions.sh uses, parameterised by its anchor: walk
# forward literal by literal and treat a "]" seen OUTSIDE a literal as the end
# of the array. It cannot search for the next "]" — the launch body itself
# contains `[ "${AG_REMEMBER:-1}" = 1 ]`, and a naive search stops inside it.

extract() {   # extract <anchor-literal-prefix> <outfile>
    python3 - "$GREETER" "$1" "$2" <<'PY'
import json, re, sys
src = open(sys.argv[1], encoding="utf-8").read()
anchor = src.find(sys.argv[2])
if anchor < 0:
    sys.exit("anchor not found in the greeter: %s" % sys.argv[2])
LIT = re.compile(r'"(?:[^"\\]|\\.)*"')
parts, i, n = [], anchor, len(src)
while i < n:
    c = src[i]
    if c == '"':
        m = LIT.match(src, i)
        if not m:
            sys.exit("unterminated string literal")
        parts.append(json.loads(m.group(0)))
        i = m.end()
        continue
    if c == "]":
        break
    if c not in " \t\r\n+":
        sys.exit("unexpected %r between literals" % c)
    i += 1
else:
    sys.exit("command array is not terminated")
open(sys.argv[3], "w", encoding="utf-8").write("".join(parts))
PY
}

LAUNCH_RAW="$WORK/launch-raw.sh"
extract '"d=/var/lib/apex-greet;' "$LAUNCH_RAW"
is "the launch hook is extractable from the shipped GreetContext.qml" "0" "$?"

launch_src="$(cat "$LAUNCH_RAW" 2>/dev/null)"
if [ "${#launch_src}" -ge 200 ]; then
    ok "…and is a whole script rather than a fragment (${#launch_src} chars)"
else
    bad "…and is a whole script rather than a fragment" "got ${#launch_src} chars"
fi
has "…and it is the code that writes last-session"       "last-session"              "$launch_src"
has "…and the watchdog record call came with it"         "apex-session-watchdog record" "$launch_src"
has "…and the record call is capped by timeout"          "timeout 5"                 "$launch_src"

# Repoint the two absolute paths and nothing else.
LAUNCH_DIR="$WORK/launch"; mkdir -p "$LAUNCH_DIR"
stage_launch() {   # stage_launch <path-to-the-helper-it-should-call>
    sed -e "s#/var/lib/apex-greet#$LAUNCH_DIR/state#g" \
        -e "s#/usr/libexec/apex-session-watchdog#$1#g" "$LAUNCH_RAW"
}
run_launch() {     # run_launch <helper> <AG_SESS> <AG_REMEMBER> [extra env...]
    local helper=$1 sess=$2 remember=$3; shift 3
    env AG_USER=andre AG_SESS="$sess" AG_REMEMBER="$remember" \
        APEX_SESSION_WATCHDOG_STATE="$LAUNCH_DIR/state" \
        APEX_SESSION_WATCHDOG_SESSIONS="$STAGE" \
        "$@" sh -c "$(stage_launch "$helper")"
}

# A helper that just records what it was called with, so the hook can be tested
# without the real one's state machine in the way.
SPY="$WORK/spy"
cat > "$SPY" <<EOF
#!/bin/sh
printf '%s\n' "\$*" >> "$WORK/spy.log"
EOF
chmod +x "$SPY"

rm -rf "$LAUNCH_DIR/state" "$WORK/spy.log"; mkdir -p "$LAUNCH_DIR/state"
printf 'niri' > "$LAUNCH_DIR/state/last-session"
run_launch "$SPY" apex-labwc 1; rcL=$?
is "the launch hook exits 0" "0" "$rcL"
is "…writes last-user" "andre" "$(cat "$LAUNCH_DIR/state/last-user" 2>/dev/null)"
is "…remembers an ordinary session" "apex-labwc" "$(cat "$LAUNCH_DIR/state/last-session" 2>/dev/null)"
is "…and tells the watchdog what it launched" "record apex-labwc" "$(cat "$WORK/spy.log" 2>/dev/null)"

# THE STICKINESS FIX. Before it, last-session was written on every login, so one
# trip through APEX Safe Graphics made the recovery desktop the preselected
# default for ever: the user fixed their machine and the picker still pointed at
# the rescue session. The previous memory has to SURVIVE, not be cleared —
# clearing it would drop them onto the named default instead of the desktop they
# were actually using.
rm -rf "$LAUNCH_DIR/state" "$WORK/spy.log"; mkdir -p "$LAUNCH_DIR/state"
printf 'niri' > "$LAUNCH_DIR/state/last-session"
run_launch "$SPY" "$RECOVERY_ID" 0
is "the recovery session is NOT remembered as the default" \
    "niri" "$(cat "$LAUNCH_DIR/state/last-session" 2>/dev/null)"
is "…the user is still remembered" "andre" "$(cat "$LAUNCH_DIR/state/last-user" 2>/dev/null)"
is "…and the watchdog is still told, so recovery's own bounces are visible" \
    "record $RECOVERY_ID" "$(cat "$WORK/spy.log" 2>/dev/null)"

# ── The three fail-open mutations ───────────────────────────────────────────
# The greeter defers Greetd.launch() to persistProc.onExited, so a hook that
# never exits is a machine nobody can log in to. Each mutation below breaks the
# helper a different way and requires the same three things: the hook returns,
# it returns 0, and the greeter's own memory was written before the helper was
# ever called.

check_fail_open() {   # check_fail_open <label> <helper> [extra env...]
    local label=$1 helper=$2; shift 2
    rm -rf "$LAUNCH_DIR/state"; mkdir -p "$LAUNCH_DIR/state"
    local start end rc
    start=$(date +%s)
    run_launch "$helper" apex-labwc 1 "$@" >/dev/null 2>&1
    rc=$?
    end=$(date +%s)
    is "FAIL OPEN — $label: the hook exits 0" "0" "$rc"
    if [ "$((end - start))" -le 10 ]; then
        ok "FAIL OPEN — $label: …within 10s (took $((end - start))s)"
    else
        bad "FAIL OPEN — $label: …within 10s" "took $((end - start))s"
    fi
    is "FAIL OPEN — $label: …and last-user was written anyway" \
       "andre" "$(cat "$LAUNCH_DIR/state/last-user" 2>/dev/null)"
    is "FAIL OPEN — $label: …and last-session was written anyway" \
       "apex-labwc" "$(cat "$LAUNCH_DIR/state/last-session" 2>/dev/null)"
}

HANG="$WORK/hang"
printf '#!/bin/sh\nsleep 60\n' > "$HANG"; chmod +x "$HANG"
check_fail_open "a helper that hangs for ever" "$HANG"

check_fail_open "a helper that is not there at all" "$WORK/definitely-absent"

NOISY="$WORK/noisy"
printf '#!/bin/sh\necho boom >&2\nexit 2\n' > "$NOISY"; chmod +x "$NOISY"
check_fail_open "a helper that fails loudly" "$NOISY"

# `timeout` is the cap, so the case where `timeout` ITSELF is missing has to
# degrade too — otherwise the belt is there and the braces are not.
NOPATH="$WORK/nopath/bin"; mkdir -p "$NOPATH"
# `sh` is on this list because `env PATH=… sh -c` resolves `sh` with the NEW
# PATH; without it the harness gets 127 from env(1) and reports a failure the
# hook never had. Everything the extracted body needs is here EXCEPT timeout.
for b in sh mkdir cat sleep; do
    p="$(command -v "$b" 2>/dev/null)" && ln -sf "$p" "$NOPATH/$b"
done
if command -v timeout >/dev/null 2>&1 && [ ! -e "$NOPATH/timeout" ]; then
    check_fail_open "no timeout(1) on PATH to cap it with" "$HANG" "PATH=$NOPATH"
else
    skp "FAIL OPEN — no timeout(1) on PATH to cap it with" "could not stage a PATH without timeout"
fi

# And the positive control: with the REAL helper on the other end, three runs of
# the greeter's own launch hook are what feed the counter.
rm -rf "$LAUNCH_DIR/state"; mkdir -p "$LAUNCH_DIR/state"
for _ in 1 2 3; do
    run_launch "$WD" apex-labwc 1 >/dev/null 2>&1
    env APEX_SESSION_WATCHDOG_STATE="$LAUNCH_DIR/state" \
        APEX_SESSION_WATCHDOG_SESSIONS="$STAGE" sh "$WD" check >/dev/null
done
is "three runs of the greeter's OWN launch hook reach the threshold" \
   "$RECOVERY_ID" \
   "$(env APEX_SESSION_WATCHDOG_STATE="$LAUNCH_DIR/state" \
          APEX_SESSION_WATCHDOG_SESSIONS="$STAGE" sh "$WD" check)"

# ─────────────────────────────────────────────────────────────────────────────
section "§3 the greeter's check hook, and what it is allowed to act on"
# ─────────────────────────────────────────────────────────────────────────────

CHECK_RAW="$WORK/check-raw.sh"
extract '"timeout 5 /usr/libexec/apex-session-watchdog check' "$CHECK_RAW"
is "the check hook is extractable from the shipped GreetContext.qml" "0" "$?"
check_src="$(cat "$CHECK_RAW" 2>/dev/null)"
has "…and it is the code that calls the watchdog" "apex-session-watchdog check" "$check_src"
has "…and it ends with the bare echo the SplitParser needs" "; echo" "$check_src"

# The greeter's side of the contract, in four lines, taken verbatim from
# GreetContext.qml's onRead: trim, and act ONLY on an id equal to
# `recoverySession`. Everything else the helper could emit selects nothing.
# §4 proves this really is what the QML does.
greeter_would_select() {   # reads hook output on stdin
    local line s out=""
    while IFS= read -r line; do
        s="$(printf '%s' "$line" | tr -d '[:space:]')"
        [ "$s" = "$RECOVERY_ID" ] && out="$s"
    done
    printf '%s' "$out"
}
run_check() {   # run_check <helper> [extra env...]
    local helper=$1; shift
    env APEX_SESSION_WATCHDOG_STATE="$STATE" \
        APEX_SESSION_WATCHDOG_SESSIONS="$STAGE" "$@" \
        sh -c "$(sed "s#/usr/libexec/apex-session-watchdog#$helper#g" "$CHECK_RAW")"
}

reset_state
is "below the threshold the greeter selects nothing" "" "$(run_check "$WD" | greeter_would_select)"
for _ in 1 2 3; do wd record apex-labwc >/dev/null; wd check >/dev/null; done
is "at the threshold the greeter selects the recovery session" \
   "$RECOVERY_ID" "$(run_check "$WD" | greeter_would_select)"

GARBAGE="$WORK/garbage"
cat > "$GARBAGE" <<'EOF'
#!/bin/sh
echo "hyprland"
echo "/etc/shadow"
echo "apex-safe-graphics-but-not-really"
echo "   "
EOF
chmod +x "$GARBAGE"
is "a helper that names OTHER sessions selects nothing" "" "$(run_check "$GARBAGE" | greeter_would_select)"

MULTI="$WORK/multi"
printf '#!/bin/sh\necho %s\necho hyprland\n' "$RECOVERY_ID" > "$MULTI"; chmod +x "$MULTI"
is "a second line cannot smuggle a different session past the guard" \
   "$RECOVERY_ID" "$(run_check "$MULTI" | greeter_would_select)"

start=$(date +%s); out="$(run_check "$HANG" 2>/dev/null | greeter_would_select)"; end=$(date +%s)
is "FAIL OPEN — a check that hangs selects nothing" "" "$out"
if [ "$((end - start))" -le 10 ]; then
    ok "FAIL OPEN — …and the greeter is not held up (took $((end - start))s)"
else
    bad "FAIL OPEN — …and the greeter is not held up" "took $((end - start))s"
fi
is "FAIL OPEN — a check helper that is not there selects nothing" \
   "" "$(run_check "$WORK/definitely-absent" 2>/dev/null | greeter_would_select)"

# ─────────────────────────────────────────────────────────────────────────────
section "§4 the two halves name the same session, and the guards are in the QML"
# ─────────────────────────────────────────────────────────────────────────────
# §3's `greeter_would_select` is a restatement of the QML, so these assertions
# are what stop it from being a restatement of something the QML no longer does.

qml_recovery="$(sed -n 's/.*readonly property string recoverySession: *"\([^"]*\)".*/\1/p' "$GREETER" | head -n1)"
is "the greeter names a recovery session" "$RECOVERY_ID" "$qml_recovery"
wd_recovery="$(sed -n 's/^RECOVERY="\([^"]*\)".*/\1/p' "$WD" | head -n1)"
is "the watchdog names the same one" "$qml_recovery" "$wd_recovery"
if [ -r "$SESSIONS_SRC/$qml_recovery.desktop" ]; then
    ok "…and that id is a session entry this repo actually ships"
else
    bad "…and that id is a session entry this repo actually ships" \
        "$SESSIONS_SRC/$qml_recovery.desktop missing"
fi

qml="$(cat "$GREETER")"
hasf "the greeter acts on the helper's output ONLY when it equals recoverySession" \
    's === ctx.recoverySession' "$qml"
hasf "cycling the picker marks the choice as the user's" \
    'ctx._userPicked     = true' "$qml"
hasf "…and clears the recovery preselection with it" \
    'ctx._recoverWanted  = ""' "$qml"
hasf "…so a later enumeration line cannot move the picker back" \
    'if (ctx._userPicked) return' "$qml"
hasf "the notice is set inside the branch that MATCHED an installed session" \
    'ctx.recoveryNotice = "Your desktop did not start' "$qml"
hasf "AG_REMEMBER is computed from recoverySession, not from a second spelling" \
    'sid !== ctx.recoverySession' "$qml"

surface="$(cat "$SURFACE")"
hasf "the surface shows the notice on the status line" \
    'root.ctx.recoveryNotice !== ""' "$surface"
hasf "…and a screen reader gets it without the glyph" \
    ': (root.ctx.recoveryNotice !== "" ? root.ctx.recoveryNotice' "$surface"
# The auth error is about the keystroke the user just made and must stay first.
hasf "…and an auth error still outranks it" \
     'text: root.ctx.hasError ? root.ctx.errorText' "$surface"

printf '\nsession-watchdog: %d passed, %d failed, %d skipped\n' "$pass" "$fail" "$skip"
[ "$fail" -eq 0 ]
