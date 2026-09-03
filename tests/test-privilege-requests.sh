#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  End-to-end assertions for structured privilege requests (roadmap §4).
#
#  The unit tests cover the vocabulary, the argument validation and the grant
#  store. What they cannot cover is the claim the whole design rests on:
#
#      the daemon learns which session is asking from the KERNEL, not from
#      anything the asking process said.
#
#  That needs a real daemon, a real session and a real socket, so it is tested
#  here against all three.
#
#  NOTHING IN THIS FILE NEEDS ROOT. Every approval uses `--no-run`, which
#  records the decision without performing the operation, so running the suite
#  never installs a package and never raises an authentication prompt. The
#  execution path is the one thing asserted only by unit tests, deliberately.
#
#      ./tests/test-privilege-requests.sh
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
WORK="$(mktemp -d)"

pass=0; fail=0
ok()  { printf 'PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL  %s\n' "$1"; fail=$((fail + 1)); }
# There is deliberately no `skp` helper. It existed for exactly one caller —
# the whole-suite skip on a missing cargo — and a suite with a skip helper
# lying around invites the next one.
section() { printf '\n── %s ──\n' "$1"; }

DAEMON_PID=""
cleanup() {
    [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null
    # Give it a moment to tear down its sessions before the tree goes away.
    [ -n "$DAEMON_PID" ] && { for _ in 1 2 3 4 5; do
        kill -0 "$DAEMON_PID" 2>/dev/null || break; sleep 0.2; done; }
    [ -n "$DAEMON_PID" ] && kill -9 "$DAEMON_PID" 2>/dev/null
    rm -rf "$WORK"
}
trap cleanup EXIT

# ── prerequisites ────────────────────────────────────────────────────────────
#
# A missing prerequisite is a FAILURE, never a skip. This suite used to
# whole-suite-skip on a missing `cargo`, print "0 passed, 0 failed (skipped)"
# and exit 0 — a green tick over nothing asserted, which is the shape
# docs/p1-progress.md already records this repository being bitten by three
# times, most recently when the labwc keybind suite reported passed=0 failed=0
# on its first CI run.
for tool in cargo git python3; do
    command -v "$tool" >/dev/null 2>&1 || {
        echo "FATAL: $tool is required; this suite cannot test anything without it" >&2
        exit 2
    }
done

# ── build ────────────────────────────────────────────────────────────────────
section "the binaries"
if ! cargo build --manifest-path "${ROOT}/apexd/Cargo.toml" \
        --bin apex-agentd --bin apex >/dev/null 2>&1; then
    bad "apex-agentd and apex build"
    printf '\nprivilege: %d passed, %d failed\n' "$pass" "$fail"
    exit 1
fi
ok "apex-agentd and apex build"

BIN="${ROOT}/apexd/target/debug"
AGENTD="${BIN}/apex-agentd"
APEX="${BIN}/apex"

# ── an isolated runtime ──────────────────────────────────────────────────────
# Separate XDG_RUNTIME_DIR and XDG_STATE_HOME, so this never touches the
# developer's own sessions, requests, grants or audit log.
export XDG_RUNTIME_DIR="${WORK}/run"
export XDG_STATE_HOME="${WORK}/state"
export XDG_CONFIG_HOME="${WORK}/config"
mkdir -p "$XDG_RUNTIME_DIR" "$XDG_STATE_HOME" "$XDG_CONFIG_HOME"
chmod 0700 "$XDG_RUNTIME_DIR"

section "the daemon"
"$AGENTD" > "${WORK}/agentd.log" 2>&1 &
DAEMON_PID=$!
SOCK="${XDG_RUNTIME_DIR}/apex-agentd/control.sock"
for _ in $(seq 1 50); do [ -S "$SOCK" ] && break; sleep 0.1; done
if [ -S "$SOCK" ]; then
    ok "the daemon came up on an isolated socket"
else
    bad "the daemon came up on an isolated socket"
    sed 's/^/      /' "${WORK}/agentd.log"
    printf '\nprivilege: %d passed, %d failed\n' "$pass" "$fail"
    exit 1
fi

# ── the vocabulary is closed ─────────────────────────────────────────────────
section "the vocabulary is closed"
for evil in exec sh bash sudo eval run; do
    out="$("$APEX" request ask "$evil" whoami --reason "trying it on" 2>&1)"
    printf '%s' "$out" | grep -q "not a privileged operation" \
        || { bad "'$evil' is refused"; continue; }
    ok "'$evil' is refused"
done

out="$("$APEX" request ask install 'clang; rm -rf /' --reason "smuggling" 2>&1)"
printf '%s' "$out" | grep -q "not a valid package name" \
    && ok "a shell metacharacter in a package name is refused" \
    || bad "a shell metacharacter in a package name is refused"

out="$("$APEX" request ask install /etc/passwd --reason "path" 2>&1)"
printf '%s' "$out" | grep -q "not a valid package name" \
    && ok "a path is not a package name" || bad "a path is not a package name"

out="$("$APEX" request ask install clang --reason "" 2>&1)"
printf '%s' "$out" | grep -q "must not be empty" \
    && ok "a request with no reason is refused" || bad "a request with no reason is refused"

# ── filing from an ordinary terminal ─────────────────────────────────────────
# This process is NOT a managed session, so the daemon must attribute the
# request to no session at all rather than guessing.
section "a request from an ordinary terminal has no session"
id="$("$APEX" request ask install clang --reason "Required to compile the project" \
        --no-wait 2>"${WORK}/ask.err")"
if [ -n "$id" ]; then
    ok "the request was filed (id ${id})"
else
    bad "the request was filed"
    sed 's/^/      /' "${WORK}/ask.err"
fi

json="$("$APEX" request list --all --json 2>/dev/null)"
printf '%s' "$json" | python3 -c "
import json,sys
rs = json.load(sys.stdin)
r = [x for x in rs if x['id'] == int('${id:-0}')][0]
assert r['verb'] == 'install', r
assert r['packages'] == ['clang'], r
assert r['decision'] == 'pending', r
assert r['session'] is None, f\"attributed to a session it did not come from: {r}\"
assert r['reason'] == 'Required to compile the project', r
" 2>"${WORK}/attr.err" \
    && ok "it is pending, unattributed, and carries the reason" \
    || { bad "it is pending, unattributed, and carries the reason"; sed 's/^/      /' "${WORK}/attr.err"; }

# ── the prompt ───────────────────────────────────────────────────────────────
section "the approval prompt"
prompt="$("$APEX" request show "$id" 2>&1)"
for want in "apex install clang" "Reason" "Required to compile the project" "Effect"; do
    printf '%s' "$prompt" | grep -qF "$want" \
        && ok "the prompt shows: ${want}" || bad "the prompt shows: ${want}"
done

# ── deciding ─────────────────────────────────────────────────────────────────
# --no-run throughout: records the decision, performs nothing, needs no root.
section "deciding"
out="$(printf 'y\n' | "$APEX" request approve "$id" --no-run 2>&1)"
printf '%s' "$out" | grep -qE "approved" \
    && ok "an unsessioned peer may approve" || { bad "an unsessioned peer may approve"; printf '      %s\n' "$out"; }

out="$(printf 'y\n' | "$APEX" request approve "$id" --no-run 2>&1)"
printf '%s' "$out" | grep -q "already" \
    && ok "a decided request cannot be re-decided" || bad "a decided request cannot be re-decided"

id2="$("$APEX" request ask pin --reason "pinning before an upgrade" --no-wait 2>/dev/null)"
out="$("$APEX" request deny "$id2" 2>&1)"
printf '%s' "$out" | grep -q "denied" && ok "a request can be denied" || bad "a request can be denied"
out="$(printf 'y\n' | "$APEX" request approve "$id2" --no-run 2>&1)"
printf '%s' "$out" | grep -q "already denied" \
    && ok "a denied request cannot be flipped to approved" \
    || { bad "a denied request cannot be flipped to approved"; printf '      %s\n' "$out"; }

# ── the audit log ────────────────────────────────────────────────────────────
section "the audit log"
LOG="${XDG_STATE_HOME}/apex/agent/privilege-audit.jsonl"
if [ -s "$LOG" ]; then
    ok "an audit log was written"
    python3 - "$LOG" <<'PY' && ok "every line is one JSON object with argv and event" \
        || bad "every line is one JSON object with argv and event"
import json,sys
n = 0
for line in open(sys.argv[1]):
    line = line.strip()
    if not line: continue
    o = json.loads(line)
    assert 'event' in o and 'argv' in o and 'ms' in o, o
    n += 1
assert n >= 3, f"expected at least requested/decided entries, got {n}"
PY
    grep -q '"event":"requested"' "$LOG" || grep -q '"event": "requested"' "$LOG" \
        && ok "the filing is recorded" || bad "the filing is recorded"
    grep -q 'decided' "$LOG" && ok "the decision is recorded" || bad "the decision is recorded"
else
    bad "an audit log was written"
fi

# ── grants ───────────────────────────────────────────────────────────────────
section "grants"
"$APEX" request grants 2>&1 | grep -q "nothing is granted" \
    && ok "no grant is created by an allow-once" || bad "no grant is created by an allow-once"

# ── the property the design rests on ─────────────────────────────────────────
# A request filed from INSIDE a managed session must be attributed to that
# session by the daemon, and that session must not be able to approve itself.
#
# The session runs a shell script that files a request and then tries to
# approve it. Nothing it does can succeed at approving; if it can, the whole
# subsystem is decoration.
section "a session cannot approve itself"
PROJ="${WORK}/project"
mkdir -p "$PROJ"
git -C "$PROJ" init -q 2>/dev/null
git -C "$PROJ" -c user.email=t@t -c user.name=t commit -q --allow-empty -m init 2>/dev/null

cat > "${WORK}/inside.sh" <<EOF
#!/bin/sh
# Runs INSIDE a managed session. \$APEX_AGENT_SESSION is set here and is
# exactly what must NOT be trusted for authorisation.
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR}"
export XDG_STATE_HOME="${XDG_STATE_HOME}"
export XDG_CONFIG_HOME="${XDG_CONFIG_HOME}"
echo "SESSION_ENV=\${APEX_AGENT_SESSION:-unset}"
inner="\$("$APEX" request ask install cmake --reason "needed by the build" --no-wait 2>/dev/null)"
echo "INNER_ID=\$inner"
echo "--- trying to approve its own request ---"
"$APEX" request approve "\$inner" --no-run 2>&1
echo "APPROVE_EXIT=\$?"
echo "--- trying to deny it ---"
"$APEX" request deny "\$inner" 2>&1
echo "DENY_EXIT=\$?"
echo "--- trying to grant itself something ---"
"$APEX" request revoke "${PROJ}" 2>&1
echo "REVOKE_EXIT=\$?"
# The negative control for peer resolution: LIE about the session id. The
# daemon must still attribute this to the real session, because it never
# reads this variable — it walks /proc from the connection's peer pid.
echo "--- filing while claiming to be a different session ---"
lied="\$(APEX_AGENT_SESSION=99999 "$APEX" request ask install ninja-build \\
          --reason "filed while lying about the session" --no-wait 2>/dev/null)"
echo "LIED_ID=\$lied"
echo "DONE"
EOF
chmod +x "${WORK}/inside.sh"

# `unrestricted` so the session can reach the built binary and this script
# without a bind allowlist; peer resolution is by /proc ancestry, which is
# identical under every policy. The confined case is covered by the sandbox
# suite in apex-agent-core.
sid="$("$APEX" agent run --agent generic --sandbox unrestricted --cwd "$PROJ" -d \
        -- /bin/sh "${WORK}/inside.sh" 2>"${WORK}/run.err" \
        | sed -n 's/^session \([0-9]\+\) .*/\1/p' | head -1)"
if [ -z "$sid" ]; then
    bad "a session started"
    sed 's/^/      /' "${WORK}/run.err"
else
    ok "a session started (id ${sid})"
    # Wait for the script to finish.
    for _ in $(seq 1 80); do
        "$APEX" agent logs "$sid" 2>/dev/null | grep -q DONE && break
        sleep 0.25
    done
    logs="$("$APEX" agent logs "$sid" 2>/dev/null)"
    printf '%s\n' "$logs" | sed 's/^/      | /'

    printf '%s' "$logs" | grep -q "SESSION_ENV=${sid}" \
        && ok "the session sees its own id in the environment" \
        || bad "the session sees its own id in the environment"

    inner="$(printf '%s' "$logs" | sed -n 's/^INNER_ID=\([0-9]*\).*/\1/p' | tr -d '\r')"
    if [ -n "$inner" ]; then
        ok "the session filed a request (id ${inner})"
        # THE assertion: the daemon attributed it from the peer credentials.
        "$APEX" request list --all --json 2>/dev/null | python3 -c "
import json,sys
rs = json.load(sys.stdin)
r = [x for x in rs if x['id'] == ${inner}][0]
assert r['session'] == ${sid}, f\"session not resolved from peer credentials: {r}\"
assert r['project'] is not None, f'project not resolved: {r}'
" 2>"${WORK}/peer.err" \
            && ok "the daemon resolved the session from the peer's credentials" \
            || { bad "the daemon resolved the session from the peer's credentials"; sed 's/^/      /' "${WORK}/peer.err"; }
    else
        bad "the session filed a request"
    fi

    printf '%s' "$logs" | grep -q "cannot decide its own privilege request" \
        && ok "the session's own approval was REFUSED" \
        || bad "the session's own approval was REFUSED"
    printf '%s' "$logs" | grep -q "cannot change its own grants" \
        && ok "the session cannot change its own grants" \
        || bad "the session cannot change its own grants"

    # The negative control for peer resolution. The session filed a request
    # with APEX_AGENT_SESSION=99999. If the daemon were reading that variable
    # — or any other client-supplied value — the record would say 99999, and a
    # lying agent could attribute its requests to someone else's session or to
    # no session at all (which is the identity permitted to APPROVE).
    lied="$(printf '%s' "$logs" | sed -n 's/^LIED_ID=\([0-9]*\).*/\1/p' | tr -d '\r')"
    if [ -n "$lied" ]; then
        "$APEX" request list --all --json 2>/dev/null | python3 -c "
import json,sys
r = [x for x in json.load(sys.stdin) if x['id'] == ${lied}][0]
assert r['session'] == ${sid}, \\
    f'a client-supplied session id was believed: {r}'
assert r['session'] != 99999, f'the daemon read APEX_AGENT_SESSION: {r}'
" 2>"${WORK}/lie.err" \
            && ok "a session lying about its id is still attributed correctly" \
            || { bad "a session lying about its id is still attributed correctly"; sed 's/^/      /' "${WORK}/lie.err"; }
    else
        bad "the lying request was filed"
    fi

    # And it is still pending afterwards — the refusal is not cosmetic.
    if [ -n "${inner:-}" ]; then
        "$APEX" request list --all --json 2>/dev/null | python3 -c "
import json,sys
r = [x for x in json.load(sys.stdin) if x['id'] == ${inner}][0]
assert r['decision'] == 'pending', f'the session changed its own decision: {r}'
assert r['executed_ms'] is None, r
" 2>/dev/null \
            && ok "its request is still pending, so the refusal was real" \
            || bad "its request is still pending, so the refusal was real"
    fi
fi

# ── allow-for-project ────────────────────────────────────────────────────────
section "allow for project"
if [ -n "${inner:-}" ]; then
    out="$(printf 'y\n' | "$APEX" request approve "$inner" --for-project --no-run 2>&1)"
    printf '%s' "$out" | grep -q "allow_for_project" \
        && ok "an approval can be scoped to the project" \
        || { bad "an approval can be scoped to the project"; printf '      %s\n' "$out"; }

    "$APEX" request grants 2>/dev/null | grep -q "install:cmake" \
        && ok "the grant is recorded against the project and the exact package" \
        || bad "the grant is recorded against the project and the exact package"

    # The point of the grant: the identical request no longer prompts.
    again="$("$APEX" request ask install cmake --reason "again" --no-wait 2>/dev/null)"
    if [ -n "$again" ]; then
        # Filed from an unsessioned peer, so it has no project and must NOT be
        # auto-granted — a grant with no project to match is not a match.
        "$APEX" request list --all --json 2>/dev/null | python3 -c "
import json,sys
r = [x for x in json.load(sys.stdin) if x['id'] == ${again}][0]
assert r['decision'] == 'pending', \\
    f'a grant matched a request with no project: {r}'
" 2>/dev/null \
            && ok "a project grant does not match a request with no project" \
            || bad "a project grant does not match a request with no project"
    fi

    # A DIFFERENT package in the same project must still prompt.
    "$APEX" request revoke "${PROJ}" >/dev/null 2>&1
    "$APEX" request grants 2>/dev/null | grep -q "nothing is granted" \
        && ok "a grant can be revoked" || bad "a grant can be revoked"
fi

printf '\nprivilege: %d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
