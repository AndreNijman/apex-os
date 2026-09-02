#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  End-to-end assertions for the APEX secret broker (roadmap §4).
#
#  §4's claim is one sentence: "agents should be able to use credentials without
#  receiving the raw secret". Everything else in the broker is plumbing. So the
#  central test here stores a SENTINEL token, uses a capability from inside a
#  real confined session, and asserts the sentinel appears in
#
#      * the command's stdout
#      * the command's stderr
#      * the session's own PTY transcript
#      * the audit log
#
#  ...in none of them. If it appears anywhere, the broker has failed at the only
#  thing it exists for.
#
#  NO NETWORK IS USED. The fixture remote points at https://127.0.0.1:1/, which
#  refuses instantly, so git fails fast and the token is sent nowhere. The point
#  is not that the push succeeds — it is that the credential stayed on the
#  daemon's side of the namespace boundary while it was attempted.
#
#  NO ROOT. Nothing here needs privilege; the broker is unprivileged by design.
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
cleanup() {
    [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null
    for _ in 1 2 3 4 5; do
        [ -n "$DAEMON_PID" ] && kill -0 "$DAEMON_PID" 2>/dev/null || break
        sleep 0.2
    done
    [ -n "$DAEMON_PID" ] && kill -9 "$DAEMON_PID" 2>/dev/null
    rm -rf "$WORK"
}
trap cleanup EXIT

section "the binaries"
command -v cargo >/dev/null 2>&1 || {
    printf 'SKIP  cargo unavailable\n\nsecret-broker: 0 passed, 0 failed (skipped)\n'; exit 0; }
cargo build --manifest-path "${ROOT}/apexd/Cargo.toml" \
    --bin apex-agentd --bin apex >/dev/null 2>&1 || {
    bad "apex-agentd and apex build"
    printf '\nsecret-broker: %d passed, %d failed\n' "$pass" "$fail"; exit 1; }
ok "apex-agentd and apex build"

BIN="${ROOT}/apexd/target/debug"
AGENTD="${BIN}/apex-agentd"
APEX="${BIN}/apex"

# ── an isolated runtime ──────────────────────────────────────────────────────
export XDG_RUNTIME_DIR="${WORK}/run"
export XDG_STATE_HOME="${WORK}/state"
export XDG_CONFIG_HOME="${WORK}/config"
mkdir -p "$XDG_RUNTIME_DIR" "$XDG_STATE_HOME" "$XDG_CONFIG_HOME"
chmod 0700 "$XDG_RUNTIME_DIR"

# THE sentinel. Distinctive enough that a grep for it cannot match by accident.
SENTINEL="apex-sentinel-7f3a91c4-do-not-leak"

section "the daemon"
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
    && bad "`list` does not print the token" || ok "\`list\` does not print the token"

STORE="${XDG_STATE_HOME}/apex/agent/secrets/demo.json"
[ -f "$STORE" ] && ok "the credential is on disk" || bad "the credential is on disk"
mode="$(stat -c '%a' "$STORE" 2>/dev/null)"
[ "$mode" = "600" ] && ok "the credential file is 0600 (is ${mode})" \
                    || bad "the credential file is 0600 (is ${mode})"
dirmode="$(stat -c '%a' "$(dirname "$STORE")" 2>/dev/null)"
[ "$dirmode" = "700" ] && ok "its directory is 0700 (is ${dirmode})" \
                       || bad "its directory is 0700 (is ${dirmode})"

printf '%s' "$("$APEX" secret list --json 2>/dev/null)" | grep -q "$SENTINEL" \
    && bad "--json does not include the token" || ok "--json does not include the token"

# ── nothing is allowed by default ────────────────────────────────────────────
section "a stored credential grants nothing"
"$APEX" secret grants 2>&1 | grep -q "nothing is granted" \
    && ok "storing a credential allows nothing" || bad "storing a credential allows nothing"

out="$(cd "$PROJ" && "$APEX" secret use demo git-fetch origin 2>&1)"
printf '%s' "$out" | grep -q "not granted" \
    && ok "an ungranted capability is refused" \
    || { bad "an ungranted capability is refused"; printf '      %s\n' "$out"; }
printf '%s' "$out" | grep -q "$SENTINEL" \
    && bad "the refusal does not leak the token" || ok "the refusal does not leak the token"

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
    && bad "the host mismatch does not leak the token" || ok "the host mismatch does not leak the token"

out="$(cd "$PROJ" && "$APEX" secret use demo git-fetch viassh 2>&1)"
printf '%s' "$out" | grep -q "not an https remote" \
    && ok "an ssh remote is refused with an explanation" \
    || { bad "an ssh remote is refused with an explanation"; printf '      %s\n' "$out"; }

out="$(cd "$PROJ" && "$APEX" secret use demo git-fetch nosuchremote 2>&1)"
printf '%s' "$out" | grep -q "no remote called" \
    && ok "an unconfigured remote is refused" || bad "an unconfigured remote is refused"

# ── THE assertion ────────────────────────────────────────────────────────────
section "the token never reaches the caller"
# A granted capability, actually attempted. git will fail — 127.0.0.1:1 refuses
# — and that is fine: what is asserted is that the credential stayed on the
# daemon's side while the attempt was made.
out="$(cd "$PROJ" && "$APEX" secret use demo git-fetch origin 2>"${WORK}/use.err")"
err="$(cat "${WORK}/use.err")"
printf '%s\n%s\n' "$out" "$err" | sed 's/^/      /' | head -8

printf '%s' "$out" | grep -q "$SENTINEL" \
    && bad "the token is not in stdout" || ok "the token is not in stdout"
printf '%s' "$err" | grep -q "$SENTINEL" \
    && bad "the token is not in stderr" || ok "the token is not in stderr"
printf '%s\n%s' "$out" "$err" | grep -qE "127\.0\.0\.1|Could not resolve|refused|unable to access" \
    && ok "the operation was genuinely attempted" \
    || bad "the operation was genuinely attempted (nothing suggests git ran)"

section "the audit log records the use and not the secret"
LOG="${XDG_STATE_HOME}/apex/agent/secret-audit.jsonl"
[ -s "$LOG" ] && ok "an audit log was written" || bad "an audit log was written"
if [ -s "$LOG" ]; then
    grep -q "$SENTINEL" "$LOG" \
        && bad "the audit log does not contain the token" \
        || ok "the audit log does not contain the token"
    grep -q '"capability": *"git-fetch"' "$LOG" \
        && ok "the capability is recorded" || bad "the capability is recorded"
    grep -q '"event": *"refused"' "$LOG" \
        && ok "refusals are recorded too" || bad "refusals are recorded too"
    python3 - "$LOG" <<'PY' && ok "every line is one JSON object" || bad "every line is one JSON object"
import json,sys
for line in open(sys.argv[1]):
    if line.strip():
        o = json.loads(line)
        assert {'ms','event','service','capability'} <= set(o), o
PY
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
echo "--- can it grant itself a capability? ---"
"${SESSION_APEX}" secret grant demo git-push 2>&1
echo "--- can it use the granted one? ---"
"${SESSION_APEX}" secret use demo git-fetch origin 2>&1 | head -4
echo "DONE"
EOF
chmod +x "${PROJ}/inside.sh"

# `project` policy, so $HOME really is masked — that is the property under test.
sid="$("$APEX" agent run --agent generic --sandbox project --cwd "$PROJ" -d \
        -- /bin/sh "${PROJ}/inside.sh" 2>"${WORK}/run.err" \
        | sed -n 's/^session \([0-9]\+\) .*/\1/p' | head -1)"
if [ -z "$sid" ]; then
    # A confined session needs bwrap and a kernel without legacy TIOCSTI. If it
    # cannot start, that is reported rather than passed over: this is the one
    # section that tests the actual boundary.
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

    printf '%s' "$logs" | grep -q "$SENTINEL" \
        && bad "the session's transcript does not contain the token" \
        || ok "the session's transcript does not contain the token"
    printf '%s' "$logs" | grep -qE "No such file|Permission denied|cannot open" \
        && ok "the credential file is unreachable from inside the sandbox" \
        || bad "the credential file is unreachable from inside the sandbox"
    printf '%s' "$logs" | grep -q "cannot change its own capabilities" \
        && ok "the session cannot grant itself a capability" \
        || bad "the session cannot grant itself a capability"

    # And the grant it attempted was not recorded.
    "$APEX" secret grants 2>/dev/null | grep -q "git-push" \
        && bad "the session's self-grant was not recorded" \
        || ok "the session's self-grant was not recorded"
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

# Nothing anywhere under the isolated state may still hold the sentinel.
if grep -rq "$SENTINEL" "$XDG_STATE_HOME" 2>/dev/null; then
    printf '      still present in: %s\n' "$(grep -rl "$SENTINEL" "$XDG_STATE_HOME" 2>/dev/null | tr '\n' ' ')"
    bad "no trace of the token remains in the state directory"
else
    ok "no trace of the token remains in the state directory"
fi

printf '\nsecret-broker: %d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
