# ─────────────────────────────────────────────────────────────────────────────
#  network-loss — the machine is asked about something on the network while it
#  has no network at all.
#
#  P1-062 criterion 1's "network loss". The fault is easy; picking a subject
#  that actually MEETS it is the whole difficulty, and the first version of
#  this case is the reason the driver now has a `case_prove_exposed` step.
#
#  ═══ THE SUBJECT THAT DID NOT WORK, AND WHY IT IS RECORDED HERE ═══
#
#  The obvious subject is `apex trust --verify`: it answers a SECURITY question
#  by contacting a registry, so under network loss its failure mode is a
#  verdict rather than an error message, and a machine reporting "no signature"
#  when the truth is "nobody could ask" has told its owner their operating
#  system is unverified on the evidence of a dropped packet.
#
#  It cannot be used. `verify.rs::fetch` short-circuits on `roots.fixture
#  .is_some()` and reads `$root/registry/<tag>/` instead of running skopeo, so
#  under the fixture root this harness requires, the subject opens no socket at
#  all. The namespace was real, the refused connect() proving it was real, and
#  the case was testing nothing — which is the exact failure mode this whole
#  unit exists to refuse, found in the harness rather than by it. The lesson is
#  in run-chaos's header under THE FOURTH ARM.
#
#  ═══ THE SUBJECT THAT DOES ═══
#
#  `apex doctor` is the one status verb in APEX that opens a socket:
#  `TcpStream::connect_timeout` to the metrics endpoint on 127.0.0.1:9723. It
#  is a loopback connection, which is what makes this case cheap and hermetic —
#  no route to the internet is needed, on this laptop or on a runner — and it
#  is still a real `connect(2)` against a real network stack, so a network
#  namespace really does take it away.
#
#  Three states, which the machine has to tell apart:
#
#    A  a namespace with a DOWN loopback   → connect fails ENETUNREACH.
#                                            Nobody could ask. Whether anything
#                                            is listening is UNKNOWN.
#    B  loopback up, nothing listening     → connect fails ECONNREFUSED.
#                                            Somebody asked; the answer was no.
#    C  loopback up, a listener bound      → connect succeeds.
#
#  A and B are different facts about the machine. Collapsing them is this
#  repository's "permission denied is not absence" on the network: `.is_ok()`
#  returns false for both, and the line a person reads was the same sentence
#  either way. That is what this case found.
#
#  INJECTION   the subject runs inside `unshare --user --map-root-user --net`,
#              a fresh network namespace whose only interface is a loopback
#              that is DOWN. Nothing is reachable, including localhost.
#  PROOF       a connect() made by the HARNESS inside the same kind of
#              namespace must fail with ENETUNREACH specifically — not merely
#              fail. `unshare` exiting 0 says unshare ran; ECONNREFUSED would
#              mean the loopback came up and the case is about to test B while
#              claiming A.
#  EXPOSURE    state C, run as the baseline: with a listener bound inside the
#              namespace the metrics check must read `ok: true`. That is the
#              proof the subject really opens that socket, in the namespace it
#              is put in, rather than answering from somewhere else. If it
#              cannot be made to say true the case reports could-not-inject,
#              because nothing it says about state A would mean anything.
#  SURVIVAL    the report must distinguish A from B — the sentence a person
#              reads under a dead network must not be the sentence they read
#              when the daemon is simply not running — and must leave the
#              fixture tree untouched.
#
#  ═══ WHAT `apex doctor` READS ═══
#
#  Stated because a reader will otherwise assume the fixture root covers the
#  whole subject, and it does not. `apex doctor` is a report on THIS machine:
#  its storage and firmware rows honour `APEX_STORAGE_ROOT` and
#  `APEX_FIRMWARE_ROOT` (which is what satisfies the harness's fixture-root
#  guard), and its hardware rows read the live /sys and /proc. That is safe
#  here for the reason it is safe in tests/test-apex-recover.sh: `apex doctor`
#  spawns nothing and writes nothing. HOME and the XDG directories are still
#  redirected into the bundle, and the corruption clause is asserted over the
#  fixture tree, which is the only tree this case may change.
# ─────────────────────────────────────────────────────────────────────────────

CASE_TITLE="the network gone while the machine is asked what it can reach"
CASE_CRITERION="1 (network loss), 3 (diagnostics, no silent corruption)"
CASE_NEEDS="apex-binary userns netns"

# The port `apex doctor` probes. Hard-coded in main.rs; named once here so the
# listener and the assertions cannot drift apart from each other.
METRICS_PORT=9723

# Every invocation shares this environment. `apex doctor` writes nothing, but
# a subject that acquired a write would write into the bundle rather than into
# the developer's home, and that is not a thing to leave to trust.
_doctor_env() {
    printf 'HOME=%s\n' "$CASE_ROOT/home"
    printf 'XDG_CONFIG_HOME=%s\n' "$CASE_ROOT/home/.config"
    printf 'XDG_STATE_HOME=%s\n' "$CASE_ROOT/home/.local/state"
    printf 'XDG_CACHE_HOME=%s\n' "$CASE_ROOT/home/.cache"
    printf 'APEX_STORAGE_ROOT=%s\n' "$CASE_ROOT/machine"
    printf 'APEX_FIRMWARE_ROOT=%s\n' "$CASE_ROOT/machine"
}

_doctor() {
    # shellcheck disable=SC2046
    env $(_doctor_env) "$APEX_BIN" doctor --json
}

case_setup() {
    mkdir -p "$CASE_ROOT/home/.config" "$CASE_ROOT/home/.local/state" \
             "$CASE_ROOT/home/.cache" "$CASE_ROOT/machine"
    command -v ip >/dev/null 2>&1 || { echo "no iproute2, so loopback cannot be brought up" >&2; return 1; }
    command -v python3 >/dev/null 2>&1 || { echo "no python3, so no listener can be bound" >&2; return 1; }
    echo "fixture home and machine root at $CASE_ROOT"
}

# ── state C: the control, and the exposure proof ────────────────────────────
#
# Deliberately NOT the host. On this laptop apexd is running and 9723 answers;
# on a runner it does not, and a baseline that depended on which machine it ran
# on would make the control meaningless on one of them. Inside the namespace
# the harness owns both facts.
case_baseline() {
    unshare --user --map-root-user --net -- bash -s -- "$APEX_BIN" "$METRICS_PORT" <<'INNER'
set -uo pipefail
bin="$1"; port="$2"
ip link set lo up || { echo "could not bring the loopback up" >&2; exit 1; }
python3 - "$port" <<'PY' &
import socket, sys, time
s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", int(sys.argv[1])))
s.listen(16)
time.sleep(60)
PY
listener=$!
# Wait for the bind rather than sleeping a guessed interval: a listener that is
# not up yet would make the control read as state B and the case would report
# could-not-inject for a reason that is the harness's own fault.
for _ in $(seq 1 100); do
    python3 - "$port" <<'PY' && break
import socket, sys
s = socket.socket(); s.settimeout(0.2)
try:
    s.connect(("127.0.0.1", int(sys.argv[1]))); sys.exit(0)
except OSError:
    sys.exit(1)
PY
    sleep 0.05
done
env HOME="$CASE_ROOT/home" XDG_CONFIG_HOME="$CASE_ROOT/home/.config" \
    XDG_STATE_HOME="$CASE_ROOT/home/.local/state" \
    XDG_CACHE_HOME="$CASE_ROOT/home/.cache" \
    APEX_STORAGE_ROOT="$CASE_ROOT/machine" APEX_FIRMWARE_ROOT="$CASE_ROOT/machine" \
    "$bin" doctor --json
rc=$?
kill "$listener" 2>/dev/null || true
exit "$rc"
INNER
}

case_inject() {
    # Nothing in the tree changes. The fault is the environment the subject is
    # run in, and it is applied around the subject in case_observe. Said out
    # loud so a reader does not go looking for a mutation that is not there.
    echo "the fault is a network namespace with a DOWN loopback, applied in case_observe"
}

case_prove() {
    command -v python3 >/dev/null 2>&1 || {
        echo "no python3, so the harness cannot prove the network is gone"; return 1; }
    # The independent observation, in the same kind of namespace the subject
    # will run in, and it insists on the ERRNO rather than on failure. A case
    # that accepted any failure here would happily run state B while its header
    # claimed state A.
    local out
    out="$(unshare --user --map-root-user --net -- python3 - "$METRICS_PORT" <<'PY' 2>&1 || true
import errno, socket, sys
s = socket.socket(); s.settimeout(2)
try:
    s.connect(("127.0.0.1", int(sys.argv[1])))
    print("CONNECTED")
except OSError as e:
    print("%s %s" % (errno.errorcode.get(e.errno, e.errno), e.strerror))
PY
)"
    case "$out" in
        *ENETUNREACH*) echo "connect() inside the namespace: $out" ;;
        *CONNECTED*)   echo "a connect() succeeded inside the namespace; the network was not removed"; return 1 ;;
        *ECONNREFUSED*) echo "connect() got $out — the loopback is UP, so this is state B, not the fault this case injects"; return 1 ;;
        *)             echo "connect() failed with an unexpected error ($out); the fault is not the one this case claims"; return 1 ;;
    esac
    return 0
}

case_observe() {
    # State A: a fresh namespace, loopback left DOWN, nothing listening. Plus
    # state B in the same run, written to its own file, because the assertion
    # that matters is that A and B READ DIFFERENTLY and that needs both.
    unshare --user --map-root-user --net -- \
        env HOME="$CASE_ROOT/home" XDG_CONFIG_HOME="$CASE_ROOT/home/.config" \
            XDG_STATE_HOME="$CASE_ROOT/home/.local/state" \
            XDG_CACHE_HOME="$CASE_ROOT/home/.cache" \
            APEX_STORAGE_ROOT="$CASE_ROOT/machine" \
            APEX_FIRMWARE_ROOT="$CASE_ROOT/machine" \
            "$APEX_BIN" doctor --json > "$CASE_DIR/observe.json" 2>"$CASE_DIR/observe.jsonerr"
    local rc=$?
    unshare --user --map-root-user --net -- bash -s -- "$APEX_BIN" <<'INNER' > "$CASE_DIR/refused.json" 2>/dev/null
set -uo pipefail
bin="$1"
ip link set lo up
env HOME="$CASE_ROOT/home" XDG_CONFIG_HOME="$CASE_ROOT/home/.config" \
    XDG_STATE_HOME="$CASE_ROOT/home/.local/state" \
    XDG_CACHE_HOME="$CASE_ROOT/home/.cache" \
    APEX_STORAGE_ROOT="$CASE_ROOT/machine" APEX_FIRMWARE_ROOT="$CASE_ROOT/machine" \
    "$bin" doctor --json
INNER
    # The diagnostic a person reads is the rendered line, so it goes to stdout
    # and into the bundle beside the documents the assertions parse.
    _chaos_metrics_line "$CASE_DIR/observe.json"  "no network at all"
    _chaos_metrics_line "$CASE_DIR/refused.json"  "network up, nothing listening"
    _chaos_metrics_line "$CASE_DIR/baseline.out"  "network up, a listener bound"
    return "$rc"
}

# Pull the one row this case is about out of a doctor document, for the human
# half of the bundle. Never used by an assertion — those parse the document.
_chaos_metrics_line() {
    local file="$1" label="$2"
    printf '%-32s %s\n' "$label:" "$(python3 - "$file" <<'PY' 2>&1
import json, sys
try:
    d = json.load(open(sys.argv[1]))
except Exception as e:
    print("<unreadable: %s>" % e); raise SystemExit
rows = [c for c in d.get("checks", []) if "9723" in c.get("check", "")]
print(rows[0] if rows else "<no metrics row>")
PY
)"
}

# The one expression that names the row, used by every assertion below.
METRICS_ROW='[c for c in d["checks"] if "9723" in c["check"]][0]'

# ── the exposure proof ──────────────────────────────────────────────────────
#
# Runs after the subject, which is the whole reason it is a separate step: the
# question "did the subject meet the fault?" cannot be asked before the subject
# has run. State C is the answer — if `apex doctor` reports the metrics
# endpoint reachable when a listener is bound INSIDE the namespace, then it
# really opened that socket there, and its answer in state A is about the
# namespace and not about something else.
case_prove_exposed() {
    local got
    got="$(python3 - "$CASE_DIR/baseline.out" <<'PY' 2>&1
import json, sys
try:
    d = json.load(open(sys.argv[1]))
except Exception as e:
    print("unreadable: %s" % e); raise SystemExit
rows = [c for c in d["checks"] if "9723" in c["check"]]
print(rows[0]["ok"] if rows else "no-metrics-row")
PY
)"
    if [[ "$got" != "True" ]]; then
        echo "with a listener bound inside the namespace the metrics check still read '$got', so nothing shows the subject opens that socket in the namespace it is given"
        return 1
    fi
    echo "state C: with a listener bound inside the namespace the metrics check reads ok=true, so the subject really connects there"
    return 0
}

case_judge() {
    # The control. Asserted as well as proven, so a reader of the bundle sees
    # it in the expectation list rather than only in exposed.out.
    expect_json "with a listener bound in the namespace the endpoint reads reachable" \
        "$CASE_DIR/baseline.out" "$METRICS_ROW['ok']" "True"

    # Both faults are a WARN, and that is right: neither A nor B is a machine
    # whose metrics endpoint is reachable. The boolean is not the defect.
    expect_json "with no network the endpoint does not read reachable" \
        "$CASE_DIR/observe.json" "$METRICS_ROW['ok']" "False"
    expect_json "with nothing listening the endpoint does not read reachable either" \
        "$CASE_DIR/refused.json" "$METRICS_ROW['ok']" "False"

    # The defect is the sentence. "Nobody could ask" and "we asked and the
    # answer was no" are different facts about the machine, and the person
    # reading `apex doctor` is reading it precisely because they do not yet
    # know which one they are in.
    local a b
    a="$(python3 -c '
import json,sys
d=json.load(open(sys.argv[1]))
print([c for c in d["checks"] if "9723" in c["check"]][0]["check"])' \
        "$CASE_DIR/observe.json" 2>/dev/null || echo "<unreadable>")"
    b="$(python3 -c '
import json,sys
d=json.load(open(sys.argv[1]))
print([c for c in d["checks"] if "9723" in c["check"]][0]["check"])' \
        "$CASE_DIR/refused.json" 2>/dev/null || echo "<unreadable>")"
    if [[ "$a" == "<unreadable>" || "$b" == "<unreadable>" ]]; then
        _chaos_fail "a doctor document could not be read, so the two states cannot be compared"
    elif [[ "$a" == "$b" ]]; then
        _chaos_fail "a machine with no network says exactly what a machine with nothing listening says: '$a'"
    else
        _chaos_pass "the two states read differently"
        _chaos_pass "  no network      : $a"
        _chaos_pass "  nothing listening: $b"
    fi

    # And the unmeasured one has to SAY it is unmeasured. A differently-worded
    # line that still asserts the endpoint is down would pass the test above
    # and still be the wrong claim.
    expect_silent_about "a machine that could not ask never states the endpoint's status as a fact" \
        "reachable on 127.0.0.1:9723" "$a"

    expect_no_corruption "asking a machine with no network changed nothing on it" \
        "$CASE_DIR/state.before" "$CASE_DIR/state.after"
}
