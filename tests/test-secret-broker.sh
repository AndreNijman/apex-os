#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  End-to-end assertions for the APEX secret broker (roadmap §3.2, §11).
#
#  The claim is one sentence: agents use credentials without receiving them.
#  Everything else is plumbing. So the central test here stores a SENTINEL
#  credential, uses a capability from inside a real confined session, and
#  asserts the sentinel appears in
#
#      * the command's stdout
#      * the command's stderr
#      * the session's own PTY transcript
#      * the audit trail
#
#  ...in none of them. If it appears anywhere, the service has failed at the
#  only thing it exists for.
#
#  TWO DAEMONS, and which one answers is the point. apex-secretd owns the store
#  and performs the operation; apex-agentd owns the session, its secret policy
#  and its project, and forwards a capability record. Neither alone can do what
#  P0-002 asks: the agent runtime runs as the user, so a store it owned would be
#  a store the agent owned.
#
#  NO NETWORK IS USED. The fixture remote points at https://127.0.0.1:1/, which
#  refuses instantly, so git fails fast and the credential is sent nowhere. The
#  point is not that the fetch succeeds — that is proven hermetically by the
#  Rust suite in apexd/apex-secretd/tests/end_to_end.rs, against a real
#  credential-checking server — it is that the credential stayed on the
#  service's side while the attempt was made.
#
#  NO ROOT. apex-secretd is started with --store and --socket and runs as the
#  invoking user, so it reports `protected: false`. That costs this suite the
#  at-rest half of the boundary, which needs a uid it does not have; the API
#  half is the same code either way.
#
#      ./tests/test-secret-broker.sh
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
section() { printf '\n── %s ──\n' "$1"; }

DAEMON_PID=""
SECRETD_PID=""
# Only ever this script's own children, by recorded pid. Never by name: the
# developer's own apex-agentd is usually running, and a previous version of a
# suite like this one killed it.
cleanup() {
    for pid in "$DAEMON_PID" "$SECRETD_PID"; do
        [ -n "$pid" ] && kill "$pid" 2>/dev/null
    done
    for _ in 1 2 3 4 5; do
        alive=0
        for pid in "$DAEMON_PID" "$SECRETD_PID"; do
            [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null && alive=1
        done
        [ "$alive" = 0 ] && break
        sleep 0.2
    done
    for pid in "$DAEMON_PID" "$SECRETD_PID"; do
        [ -n "$pid" ] && kill -9 "$pid" 2>/dev/null
    done
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
#
# The one legitimate skip in this file is the sandbox section further down: a
# confined session needs bubblewrap, and where bwrap is genuinely absent the
# boundary cannot be tested at all. That skip is loud, names bubblewrap, and is
# refused outright when APEX_REQUIRE_SANDBOX is set — which CI sets, so the job
# cannot go green having skipped it.
for tool in cargo git python3; do
    command -v "$tool" >/dev/null 2>&1 || {
        echo "FATAL: $tool is required; this suite cannot test anything without it" >&2
        exit 2
    }
done

section "the binaries"
cargo build --manifest-path "${ROOT}/apexd/Cargo.toml" \
    --bin apex-agentd --bin apex-secretd --bin apex >/dev/null 2>&1 || {
    bad "apex-agentd, apex-secretd and apex build"
    printf '\nsecret-broker: %d passed, %d failed\n' "$pass" "$fail"; exit 1; }
ok "apex-agentd, apex-secretd and apex build"

BIN="${CARGO_TARGET_DIR:-${ROOT}/apexd/target}/debug"
AGENTD="${BIN}/apex-agentd"
SECRETD="${BIN}/apex-secretd"
APEX="${BIN}/apex"

# ── an isolated runtime ──────────────────────────────────────────────────────
export XDG_RUNTIME_DIR="${WORK}/run"
export XDG_STATE_HOME="${WORK}/state"
export XDG_CONFIG_HOME="${WORK}/config"
mkdir -p "$XDG_RUNTIME_DIR" "$XDG_STATE_HOME" "$XDG_CONFIG_HOME"
chmod 0700 "$XDG_RUNTIME_DIR"

# THE sentinel. Distinctive enough that a grep for it cannot match by accident.
SENTINEL="apex-sentinel-7f3a91c4-do-not-leak"

section "the secret service"
# Its own socket and its own store, both under $WORK. The variable is what the
# CLI and apex-agentd both read, and apex-agentd is started afterwards so it
# inherits it.
export APEX_SECRETD_SOCKET="${WORK}/secretd.sock"
SECRET_STORE="${WORK}/secretd-store"
"$SECRETD" --socket "$APEX_SECRETD_SOCKET" --store "$SECRET_STORE" \
    > "${WORK}/secretd.log" 2>&1 &
SECRETD_PID=$!
for _ in $(seq 1 50); do [ -S "$APEX_SECRETD_SOCKET" ] && break; sleep 0.1; done
[ -S "$APEX_SECRETD_SOCKET" ] && ok "the secret service came up" || {
    bad "the secret service came up"
    sed 's/^/      /' "${WORK}/secretd.log"
    printf '\nsecret-broker: %d passed, %d failed\n' "$pass" "$fail"; exit 1; }

# Every local account must be able to reach it; who they are is decided from
# SO_PEERCRED, not from a mode bit.
sockmode="$(stat -c '%a' "$APEX_SECRETD_SOCKET" 2>/dev/null)"
[ "$sockmode" = "666" ] && ok "the socket is reachable by any local account (is ${sockmode})" \
                        || bad "the socket is reachable by any local account (is ${sockmode})"

section "the agent runtime"
"$AGENTD" > "${WORK}/agentd.log" 2>&1 &
DAEMON_PID=$!
SOCK="${XDG_RUNTIME_DIR}/apex-agentd/control.sock"
for _ in $(seq 1 50); do [ -S "$SOCK" ] && break; sleep 0.1; done
[ -S "$SOCK" ] && ok "the daemon came up on an isolated socket" || {
    bad "the daemon came up on an isolated socket"
    sed 's/^/      /' "${WORK}/agentd.log"
    printf '\nsecret-broker: %d passed, %d failed\n' "$pass" "$fail"; exit 1; }

# ── a project with an https remote that refuses instantly ────────────────────
PROJ="${WORK}/demo"
mkdir -p "$PROJ"
git -C "$PROJ" init -q
git -C "$PROJ" -c user.email=t@t -c user.name=t commit -q --allow-empty -m init
# 127.0.0.1:1 is closed on every machine, so git fails in milliseconds and the
# token is transmitted to nothing.
git -C "$PROJ" remote add origin "https://127.0.0.1:1/demo.git"
git -C "$PROJ" remote add elsewhere "https://example.invalid/other.git"
git -C "$PROJ" remote add viassh "git@127.0.0.1:demo.git"

# ── storing ──────────────────────────────────────────────────────────────────
section "storing a credential"
printf %s "$SENTINEL" | "$APEX" secret add demo --host 127.0.0.1 >/dev/null 2>&1
out="$("$APEX" secret list 2>&1)"
printf '%s' "$out" | grep -q "demo" \
    && ok "the service is listed" || { bad "the service is listed"; printf '      %s\n' "$out"; }
printf '%s' "$out" | grep -q "$SENTINEL" \
    && bad "\`list\` does not print the credential" || ok "\`list\` does not print the credential"

# The credential lives in the secret service's store, in a file of its own, and
# NOT beside the session records. That split is what lets the wire types refuse
# to serialise a value at all: nothing hands one to serde.
STORE="${SECRET_STORE}/users/$(id -u)/demo.secret"
META="${SECRET_STORE}/users/$(id -u)/demo.json"
[ -f "$STORE" ] && ok "the credential is in the service's store" \
                || bad "the credential is in the service's store"
grep -q "$SENTINEL" "$META" 2>/dev/null \
    && bad "the metadata record holds no credential" \
    || ok "the metadata record holds no credential"
grep -rq "$SENTINEL" "$XDG_STATE_HOME" 2>/dev/null \
    && bad "nothing under the agent runtime's state holds the credential" \
    || ok "nothing under the agent runtime's state holds the credential"

mode="$(stat -c '%a' "$STORE" 2>/dev/null)"
[ "$mode" = "600" ] && ok "the credential file is 0600 (is ${mode})" \
                    || bad "the credential file is 0600 (is ${mode})"
dirmode="$(stat -c '%a' "$(dirname "$STORE")" 2>/dev/null)"
[ "$dirmode" = "700" ] && ok "its directory is 0700 (is ${dirmode})" \
                       || bad "its directory is 0700 (is ${dirmode})"

printf '%s' "$("$APEX" secret list --json 2>/dev/null)" | grep -q "$SENTINEL" \
    && bad "--json does not include the credential" || ok "--json does not include the credential"

# ── nothing is allowed by default ────────────────────────────────────────────
section "a stored credential grants nothing"
"$APEX" secret grants 2>&1 | grep -q "nothing is granted" \
    && ok "storing a credential allows nothing" || bad "storing a credential allows nothing"

out="$(cd "$PROJ" && "$APEX" secret use demo git-fetch origin 2>&1)"
printf '%s' "$out" | grep -q "not granted" \
    && ok "an ungranted capability is refused" \
    || { bad "an ungranted capability is refused"; printf '      %s\n' "$out"; }
printf '%s' "$out" | grep -q "$SENTINEL" \
    && bad "the refusal does not leak the credential" \
    || ok "the refusal does not leak the credential"

# ── the vocabulary is closed ─────────────────────────────────────────────────
section "the vocabulary is closed"
for evil in exec sh git-clone curl run; do
    out="$(cd "$PROJ" && "$APEX" secret use demo "$evil" origin 2>&1)"
    printf '%s' "$out" | grep -q "not a capability" \
        && ok "'$evil' is not a capability" || bad "'$evil' is not a capability"
done

section "a remote may not be a URL"
# The hole this closes: with a URL accepted, a session asks the broker to push
# to a host it controls and the broker does it, with the token attached.
for evil in "https://attacker.example/r" "git@github.com:a/b" "-f" "--force" "../x" "a b"; do
    out="$(cd "$PROJ" && "$APEX" secret use demo git-fetch "$evil" 2>&1)"
    printf '%s' "$out" | grep -qE "not a git remote name" \
        && ok "refused as a remote: ${evil}" \
        || { bad "refused as a remote: ${evil}"; printf '      %s\n' "$out"; }
done

# ── granting ─────────────────────────────────────────────────────────────────
section "granting"
out="$(cd "$PROJ" && "$APEX" secret grant demo git-fetch 2>&1)"
printf '%s' "$out" | grep -q "allowed demo:git-fetch" \
    && ok "a capability can be granted for the project" \
    || { bad "a capability can be granted for the project"; printf '      %s\n' "$out"; }

out="$(cd "$PROJ" && "$APEX" secret grant nosuchservice git-fetch 2>&1)"
printf '%s' "$out" | grep -q "no credential stored" \
    && ok "a grant for an unknown service is refused, not silently stored" \
    || bad "a grant for an unknown service is refused, not silently stored"

# A grant is per capability: git-fetch does not imply git-push.
out="$(cd "$PROJ" && "$APEX" secret use demo git-push origin 2>&1)"
printf '%s' "$out" | grep -q "not granted" \
    && ok "granting git-fetch does not allow git-push" \
    || { bad "granting git-fetch does not allow git-push"; printf '      %s\n' "$out"; }

section "a remote must point where the credential is for"
out="$(cd "$PROJ" && "$APEX" secret use demo git-fetch elsewhere 2>&1)"
printf '%s' "$out" | grep -q "example.invalid" \
    && ok "a remote on another host is refused" \
    || { bad "a remote on another host is refused"; printf '      %s\n' "$out"; }
printf '%s' "$out" | grep -q "$SENTINEL" \
    && bad "the host mismatch does not leak the credential" \
    || ok "the host mismatch does not leak the credential"

out="$(cd "$PROJ" && "$APEX" secret use demo git-fetch viassh 2>&1)"
printf '%s' "$out" | grep -q "not an http remote" \
    && ok "an ssh remote is refused with an explanation" \
    || { bad "an ssh remote is refused with an explanation"; printf '      %s\n' "$out"; }

out="$(cd "$PROJ" && "$APEX" secret use demo git-fetch nosuchremote 2>&1)"
printf '%s' "$out" | grep -q "no remote called" \
    && ok "an unconfigured remote is refused" || bad "an unconfigured remote is refused"

# ── THE assertion ────────────────────────────────────────────────────────────
section "the credential never reaches the caller"
# A granted capability, actually attempted. git will fail — 127.0.0.1:1 refuses
# — and that is fine: what is asserted is that the credential stayed on the
# daemon's side while the attempt was made.
out="$(cd "$PROJ" && "$APEX" secret use demo git-fetch origin 2>"${WORK}/use.err")"
err="$(cat "${WORK}/use.err")"
printf '%s\n%s\n' "$out" "$err" | sed 's/^/      /' | head -8

printf '%s' "$out" | grep -q "$SENTINEL" \
    && bad "the credential is not in stdout" || ok "the credential is not in stdout"
printf '%s' "$err" | grep -q "$SENTINEL" \
    && bad "the credential is not in stderr" || ok "the credential is not in stderr"
printf '%s\n%s' "$out" "$err" | grep -qE "127\.0\.0\.1|Could not resolve|refused|unable to access" \
    && ok "the operation was genuinely attempted" \
    || bad "the operation was genuinely attempted (nothing suggests git ran)"

section "the audit trail records the use and not the credential"
# The trail lives with the store, not with the caller: in the image that
# directory is root-owned, so the audited party cannot rewrite the audit.
LOG="${SECRET_STORE}/audit.jsonl"
[ -s "$LOG" ] && ok "an audit trail was written" || bad "an audit trail was written"
if [ -s "$LOG" ]; then
    grep -q "$SENTINEL" "$LOG" \
        && bad "the audit trail does not contain the credential" \
        || ok "the audit trail does not contain the credential"
    grep -q '"operation":"git-fetch"' "$LOG" \
        && ok "the capability is recorded" || bad "the capability is recorded"
    grep -q '"event":"refused"' "$LOG" \
        && ok "refusals are recorded too" || bad "refusals are recorded too"
    grep -q '"event":"stored"' "$LOG" \
        && ok "storing a credential is recorded too" \
        || bad "storing a credential is recorded too"
    python3 - "$LOG" <<'PYEOF' && ok "every line carries the record from section 11" || bad "every line carries the record from section 11"
import json, sys
want = {'audit_id', 'ms', 'event', 'uid', 'peer_pid', 'provider', 'operation',
        'detail', 'resource', 'project', 'agent_session', 'request_origin',
        'origin_source', 'approval_policy', 'constraints'}
for line in open(sys.argv[1]):
    if line.strip():
        o = json.loads(line)
        missing = want - set(o)
        assert not missing, (missing, o)
PYEOF
    "$APEX" secret audit 2>&1 | grep -q "git fetch origin" \
        && ok "\`apex secret audit\` reads the trail back" \
        || bad "\`apex secret audit\` reads the trail back"
    "$APEX" secret audit 2>&1 | grep -q "$SENTINEL" \
        && bad "\`apex secret audit\` does not print the credential" \
        || ok "\`apex secret audit\` does not print the credential"

    # P0-013's provenance has to survive the store moving. A trail that says
    # WHERE a request came from but not whether the daemon observed that or
    # something asked for it cannot answer the only question it is for.
    grep -q '"origin_source":"observed"' "$LOG" \
        && ok "the trail says how the origin was arrived at" \
        || { bad "the trail says how the origin was arrived at"
             grep -o '"origin_source":"[^"]*"' "$LOG" | sort -u | sed 's/^/      /'; }
    python3 - "$LOG" <<'PYEOF2' && ok "no line claims a local origin it did not observe" || bad "no line claims a local origin it did not observe"
import json, sys
for line in open(sys.argv[1]):
    if not line.strip():
        continue
    o = json.loads(line)
    if o['request_origin'] in ('local-terminal', 'apex-shell'):
        assert o['origin_source'] == 'observed', o
PYEOF2
fi

# ── from inside a confined session ───────────────────────────────────────────
section "a confined session cannot read the credential, and cannot grant itself"
# The dev binary lives in the apex-os checkout, which a `project` sandbox for a
# DIFFERENT project does not bind — so the session cannot reach it, which is the
# sandbox working correctly. In a real image `apex` is at /usr/bin/apex and is
# covered by the read-only root bind. Copying it into the project reproduces
# that reachability without weakening the policy under test.
cp "$APEX" "${PROJ}/apex"
SESSION_APEX="${PROJ}/apex"

# INSIDE the project, not in /tmp: a `project` sandbox replaces /tmp with a
# fresh tmpfs, so a script there is simply not visible and the session dies with
# "No such file or directory" — which looks exactly like a broker failure.
cat > "${PROJ}/inside.sh" <<EOF
#!/bin/sh
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR}"
export XDG_STATE_HOME="${XDG_STATE_HOME}"
export XDG_CONFIG_HOME="${XDG_CONFIG_HOME}"
cd "${PROJ}" || exit 1
echo "--- can the session read the credential file directly? ---"
cat "${STORE}" 2>&1 | head -3
echo "--- can it reach the secret service directly? ---"
"${SESSION_APEX}" secret list 2>&1 | head -3
echo "--- can it grant itself a capability? ---"
"${SESSION_APEX}" secret grant demo git-push 2>&1
echo "--- can it use the granted one? ---"
"${SESSION_APEX}" secret use demo git-fetch origin 2>&1 | head -4
echo "--- which git does the session find first? ---"
command -v git
echo "--- does the shim pass anything else through? ---"
git --version 2>&1 | head -1
echo "--- does a plain git fetch reach the broker? ---"
APEX_GIT_SERVICE=demo git fetch origin 2>&1 | head -3
echo "DONE"
EOF
chmod +x "${PROJ}/inside.sh"

# `project` policy, so $HOME really is masked — that is the property under test.
# A confined session needs bubblewrap. Where it is genuinely absent this
# section is SKIPPED — loudly, and only for that reason — because the boundary
# cannot be tested without a sandbox and reporting "failed" would be a lie
# about what was checked. CI installs bwrap precisely so this does not skip
# there; a skip in CI is itself a signal that the install step was lost.
if [ ! -x /usr/bin/bwrap ]; then
    # APEX_REQUIRE_SANDBOX turns the one legitimate skip in this file into a
    # failure, and CI sets it. Without it the `engine` job could go green on a
    # runner where the bubblewrap install step was removed or silently failed,
    # having never run the assertion §4 exists for — the same "a skipped check
    # counts as success" shape the rest of this suite no longer has.
    if [ -n "${APEX_REQUIRE_SANDBOX:-}" ]; then
        bad "bubblewrap is present (APEX_REQUIRE_SANDBOX is set)"
        printf '      /usr/bin/bwrap is missing, so the sandbox boundary — the\n'
        printf '      one thing §4 claims — cannot be tested. Refusing to skip.\n'
        printf '\nsecret-broker: %d passed, %d failed\n' "$pass" "$fail"
        exit 1
    fi
    printf 'SKIP  a confined session needs bubblewrap, which is not installed\n'
    printf '      (the sandbox-boundary assertions below cannot run here)\n'
    sid=""
    SKIPPED_SANDBOX=1
else
    SKIPPED_SANDBOX=0
    sid="$("$APEX" agent run --agent generic --sandbox project --cwd "$PROJ" -d \
            -- /bin/sh "${PROJ}/inside.sh" 2>"${WORK}/run.err" \
            | sed -n 's/^session \([0-9]\+\) .*/\1/p' | head -1)"
fi

if [ "$SKIPPED_SANDBOX" = 1 ]; then
    :
elif [ -z "$sid" ]; then
    # bwrap IS present and the session still did not start. That is a real
    # failure, not an environment limitation.
    bad "a confined session started"
    sed 's/^/      /' "${WORK}/run.err"
else
    ok "a confined session started (id ${sid})"
    for _ in $(seq 1 100); do
        "$APEX" agent logs "$sid" 2>/dev/null | grep -q DONE && break
        sleep 0.25
    done
    logs="$("$APEX" agent logs "$sid" 2>/dev/null)"
    printf '%s\n' "$logs" | sed 's/^/      | /' | head -20

    # THE SCRIPT MUST HAVE RUN. Without this gate every assertion below passes
    # vacuously when the sandbox fails to build — which is exactly what
    # happened on a runner where bubblewrap installed but could not create a
    # user namespace ("setting up uid map: Permission denied"). The session
    # record existed, so "started" passed; the script never executed, so
    # "the credential file is unreachable" passed because nothing tried to
    # read it. A test that passes for the wrong reason is worse than one that
    # fails.
    if ! printf '%s' "$logs" | grep -q DONE; then
        bad "the session's script actually ran"
        printf '      the sandbox did not come up, so nothing below was tested\n'
        printf '      (a "uid map: Permission denied" here means unprivileged\n'
        printf '       user namespaces are blocked — see the CI sysctl)\n'
    else
        ok "the session's script actually ran"

        printf '%s' "$logs" | grep -q "$SENTINEL" \
            && bad "the session's transcript does not contain the credential" \
            || ok "the session's transcript does not contain the credential"
        printf '%s' "$logs" | grep -qE "No such file|Permission denied|cannot open" \
            && ok "the credential file is unreachable from inside the sandbox" \
            || bad "the credential file is unreachable from inside the sandbox"
        # Two locks on the same door, and both are asserted because either one
        # alone is a line away from being removed. The sandbox masks /run and
        # binds back only the agent runtime's own socket, so a confined session
        # cannot open the secret service at all; and the service refuses a
        # mutating verb from any caller inside a session, which is what covers
        # an UNCONFINED one.
        printf '%s' "$logs" | grep -qE "secret service is not running|cannot reach the secret service" \
            && ok "the secret service is unreachable from inside the sandbox" \
            || bad "the secret service is unreachable from inside the sandbox"
        printf '%s' "$logs" | grep -qE "cannot change its own capabilities|agent session cannot|secret service is not running|cannot reach the secret service" \
            && ok "the session cannot grant itself a capability" \
            || bad "the session cannot grant itself a capability"

        # And the grant it attempted was not recorded.
        "$APEX" secret grants 2>/dev/null | grep -q "git-push" \
            && bad "the session's self-grant was not recorded" \
            || ok "the session's self-grant was not recorded"

        # ── §12: a skill's own `git` reaches the broker ──────────────────
        #
        # The point of the shim is that nothing had to be rewritten. `git
        # fetch` is what a skill types; what it must produce is the broker's
        # answer and not git's own "could not read Username".
        printf '%s' "$logs" | grep -q 'bin/git' \
            && ok "the session's git is the shim, not /usr/bin/git" \
            || bad "the session's git is the shim, not /usr/bin/git"
        printf '%s' "$logs" | grep -q 'git version' \
            && ok "and the shim passes everything else through to the real git" \
            || bad "and the shim passes everything else through to the real git"
        printf '%s' "$logs" | grep -q 'against https://127.0.0.1' \
            && ok "a plain git fetch was performed by the broker" \
            || bad "a plain git fetch was performed by the broker"
        printf '%s' "$logs" | grep -qi 'could not read Username\|terminal prompts disabled' \
            && bad "git never asked the session for a credential" \
            || ok "git never asked the session for a credential"
    fi
fi

# ── revoke ───────────────────────────────────────────────────────────────────
section "revoking"
out="$(cd "$PROJ" && "$APEX" secret revoke demo git-fetch 2>&1)"
printf '%s' "$out" | grep -q "withdrew" \
    && ok "a capability can be withdrawn" || bad "a capability can be withdrawn"
out="$(cd "$PROJ" && "$APEX" secret use demo git-fetch origin 2>&1)"
printf '%s' "$out" | grep -q "not granted" \
    && ok "a withdrawn capability is refused again" || bad "a withdrawn capability is refused again"

section "removing"
"$APEX" secret remove demo >/dev/null 2>&1
[ ! -f "$STORE" ] && ok "removing deletes the stored credential" \
                  || bad "removing deletes the stored credential"

# Nothing anywhere under either daemon's state may still hold the sentinel. The
# audit trail is deliberately in scope: it outlives the credential, and if the
# credential were in it, deleting the credential would not have deleted it.
if grep -rq "$SENTINEL" "$SECRET_STORE" "$XDG_STATE_HOME" 2>/dev/null; then
    printf '      still present in: %s\n' "$(grep -rl "$SENTINEL" "$SECRET_STORE" "$XDG_STATE_HOME" 2>/dev/null | tr '\n' ' ')"
    bad "no trace of the credential remains on disk"
else
    ok "no trace of the credential remains on disk"
fi

printf '\nsecret-broker: %d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
