#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  End-to-end assertions for the agent profile system (roadmap §5, P0-009) and
#  its read-only mounts (P0-010).
#
#  The unit tests cover the classification table, the redaction and the bundle
#  format. What they cannot cover is the two claims the design rests on:
#
#      a portable bundle carries no credential and no conversation, from a
#      profile on a real filesystem; and
#
#      a confined session cannot rewrite the instructions it was started with,
#      enforced by the kernel rather than by an argument list.
#
#  Both need a real profile tree, a real daemon and a real mount namespace, so
#  both are asserted here against all three.
#
#  NOTHING HERE TOUCHES YOUR OWN PROFILE. The suite builds a fixture home under
#  /var/tmp and runs with HOME, XDG_RUNTIME_DIR, XDG_STATE_HOME and
#  XDG_CONFIG_HOME all pointing inside it. /var/tmp and not /tmp because the
#  sandbox masks /tmp with a tmpfs, and a fixture the session cannot see would
#  be a suite asserting nothing.
#
#  The agent binary is a stub on PATH. `claude` is resolved through PATH at
#  spawn time, which is what lets a user's own build win — and what lets this
#  suite exercise the real mount code without an account, a network or an API
#  call.
#
#      ./tests/test-agent-profile.sh
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
# Deliberate, and the same reason test-privilege-requests.sh gives: this suite
# counts failures rather than aborting, and several assertions run commands
# that exit non-zero on purpose. Under `bash -e {0}`, which is how GitHub
# Actions invokes a script, `x="$(cmd)"` with a non-zero cmd kills the run.
set +e

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d -p /var/tmp apex-profile-XXXXXX)"

pass=0; fail=0
ok()  { printf 'PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL  %s\n' "$1"; fail=$((fail + 1)); }
section() { printf '\n── %s ──\n' "$1"; }

DAEMON_PID=""
cleanup() {
    if [ -n "$DAEMON_PID" ]; then
        kill "$DAEMON_PID" 2>/dev/null
        for _ in 1 2 3 4 5; do kill -0 "$DAEMON_PID" 2>/dev/null || break; sleep 0.2; done
        kill -9 "$DAEMON_PID" 2>/dev/null
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT

# ── prerequisites ────────────────────────────────────────────────────────────
# A missing prerequisite is a failure, never a skip. A suite that reports
# "0 passed, 0 failed" is a green tick over nothing asserted.
for tool in cargo python3 bwrap; do
    command -v "$tool" >/dev/null 2>&1 || {
        echo "FATAL: $tool is required; this suite cannot test anything without it" >&2
        exit 2
    }
done

section "the binaries"
if ! cargo build --manifest-path "${ROOT}/apexd/Cargo.toml" \
        --bin apex-agentd --bin apex >/dev/null 2>&1; then
    bad "apex-agentd and apex build"
    printf '\nprofile: %d passed, %d failed\n' "$pass" "$fail"
    exit 1
fi
ok "apex-agentd and apex build"

BIN="${CARGO_TARGET_DIR:-${ROOT}/apexd/target}/debug"
AGENTD="${BIN}/apex-agentd"
APEX="${BIN}/apex"

# ── a fixture profile ────────────────────────────────────────────────────────
# The shape of a real ~/.claude: instructions, a skill, a slash command, a
# status line, marketplace definitions, and beside them the three things a
# bundle must never carry — a transcript, a shell snapshot and a credential.
section "a fixture profile"
export HOME="${WORK}/home"
export XDG_RUNTIME_DIR="${WORK}/run"
export XDG_STATE_HOME="${WORK}/state"
export XDG_CONFIG_HOME="${WORK}/config"
mkdir -p "$XDG_RUNTIME_DIR" "$XDG_STATE_HOME" "$XDG_CONFIG_HOME"
chmod 0700 "$XDG_RUNTIME_DIR"

C="${HOME}/.claude"
mkdir -p "${C}/skills/demo" "${C}/commands" "${C}/projects/proj" \
         "${C}/shell-snapshots" "${C}/plugins/marketplaces/mkt" "${C}/daemon"
printf 'be brief\n'                      > "${C}/CLAUDE.md"
printf '# demo skill\n'                  > "${C}/skills/demo/SKILL.md"
printf 'go\n'                            > "${C}/commands/go.md"
printf '#!/bin/sh\necho status\n'        > "${C}/statusline.sh"
chmod 0755 "${C}/statusline.sh"
printf '{"conversation":"PRIVATE-TRANSCRIPT"}\n' > "${C}/projects/proj/chat.jsonl"
printf 'export SHELL_STATE=PRIVATE-SNAPSHOT\n'   > "${C}/shell-snapshots/snap.sh"
printf '{"claudeAiOauth":"PRIVATE-OAUTH"}\n'     > "${C}/.credentials.json"
printf 'PRIVATE-DAEMON-KEY\n'                    > "${C}/daemon/control.key"
cat > "${C}/settings.json" <<'JSON'
{
  "model": "opus",
  "env": {"GITHUB_TOKEN": "ghp_PRIVATE-ENV-VALUE"},
  "enabledPlugins": {"demo@mkt": true},
  "statusLine": {"type": "command", "command": "~/.claude/statusline.sh"},
  "hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": "true"}]}]}
}
JSON
cat > "${C}/plugins/known_marketplaces.json" <<JSON
{"mkt": {"source": {"source": "github", "repo": "acme/mkt"},
         "installLocation": "${C}/plugins/marketplaces/mkt",
         "lastUpdated": "2026-01-01T00:00:00Z"}}
JSON
cat > "${HOME}/.claude.json" <<'JSON'
{
  "userID": "PRIVATE-USER-ID",
  "machineID": "PRIVATE-MACHINE-ID",
  "oauthAccount": {"emailAddress": "PRIVATE-EMAIL"},
  "mcpServers": {
    "vault": {"type": "http", "url": "https://vault.example/mcp",
              "headers": {"Authorization": "Bearer PRIVATE-BEARER"}},
    "memory": {"command": "npx", "args": ["-y", "server"],
               "env": {"API": "PRIVATE-MCP-ENV"}}
  }
}
JSON
ok "the fixture profile is in place"

# Every string that must never leave the machine. Asserted as a set rather
# than one by one, so a bundle format that grew a new file is covered by the
# same check.
SECRETS=(PRIVATE-TRANSCRIPT PRIVATE-SNAPSHOT PRIVATE-OAUTH PRIVATE-DAEMON-KEY
         PRIVATE-ENV-VALUE PRIVATE-USER-ID PRIVATE-MACHINE-ID PRIVATE-EMAIL
         PRIVATE-BEARER PRIVATE-MCP-ENV)

# ── list and inspect ─────────────────────────────────────────────────────────
section "list and inspect"
out="$("$APEX" agent profile list 2>&1)"
printf '%s' "$out" | grep -q '^claude' \
    && ok "list names the claude profile" || { bad "list names the claude profile"; echo "$out"; }

"$APEX" agent profile inspect claude --json > "${WORK}/inspect.json" 2>"${WORK}/inspect.err"
python3 - "${WORK}/inspect.json" <<'PY' > "${WORK}/inspect.out" 2>&1
import json, sys
rows = {r["path"]: r for r in json.load(open(sys.argv[1]))}
want = {
    "~/.claude/CLAUDE.md":     ("reusable", "read-only"),
    "~/.claude/skills":        ("reusable", "read-only"),
    "~/.claude/commands":      ("reusable", "read-only"),
    "~/.claude/settings.json": ("mixed",    "read-only"),
    "~/.claude/projects":      ("machine-local", "writable"),
    "~/.claude/shell-snapshots": ("machine-local", "writable"),
    "~/.claude/.credentials.json": ("secret", "writable"),
}
bad = [f"{p}: {rows.get(p)} wanted {v}" for p, v in want.items()
       if (rows.get(p, {}).get("class"), rows.get(p, {}).get("mount")) != v]
print("\n".join(bad) if bad else "ok")
# The profile root is never itself an entry: binding the directory is what
# P0-010 replaced.
print("root-bound" if "~/.claude" in rows else "ok")
PY
grep -qx 'ok' "${WORK}/inspect.out" && [ "$(sort -u "${WORK}/inspect.out" | tr -d '\n')" = "ok" ] \
    && ok "inspect gives every entry a class and a mount" \
    || { bad "inspect gives every entry a class and a mount"; cat "${WORK}/inspect.out"; }

# ── doctor ───────────────────────────────────────────────────────────────────
section "doctor"
out="$("$APEX" agent profile doctor claude 2>&1)"; rc=$?
missing=""
for want in config statusline hooks commands skills plugins mcp credentials; do
    printf '%s' "$out" | grep -qx "$want" || missing="${missing} ${want}"
done
[ -z "$missing" ] && ok "doctor reports config, hooks, plugins, MCP and skills" \
    || { bad "doctor is missing sections:${missing}"; echo "$out"; }

printf '%s' "$out" | grep -q '1 skills' \
    && ok "doctor counts the skills" || { bad "doctor counts the skills"; echo "$out"; }
printf '%s' "$out" | grep -q 'SessionStart' \
    && ok "doctor names the hook events" || { bad "doctor names the hook events"; echo "$out"; }
printf '%s' "$out" | grep -q 'vault.*needs Authorization' \
    && ok "doctor says an HTTP MCP server needs its header" \
    || { bad "doctor says an HTTP MCP server needs its header"; echo "$out"; }
printf '%s' "$out" | grep -q '.credentials.json' && printf '%s' "$out" | grep -q 'excluded' \
    && ok "doctor names the credentials and says they are not exported" \
    || { bad "doctor names the credentials and says they are not exported"; echo "$out"; }
[ "$rc" = 0 ] && ok "a healthy profile exits zero" || { bad "a healthy profile exits zero (got $rc)"; echo "$out"; }

# A skill directory with no SKILL.md does not load, and nothing upstream says so.
mkdir -p "${C}/skills/broken"
out="$("$APEX" agent profile doctor claude 2>&1)"; rc=$?
[ "$rc" != 0 ] && printf '%s' "$out" | grep -q 'broken' \
    && ok "a skill that will not load is a problem and a non-zero exit" \
    || { bad "a skill that will not load is a problem and a non-zero exit"; echo "$out"; }
rmdir "${C}/skills/broken"

out="$("$APEX" agent profile doctor codex 2>&1)"
printf '%s' "$out" | grep -q 'no profile description' \
    && ok "an agent APEX has no profile for is refused by name" \
    || { bad "an agent APEX has no profile for is refused by name"; echo "$out"; }

# ── export ───────────────────────────────────────────────────────────────────
section "export"
BUNDLE="${WORK}/bundle"
out="$("$APEX" agent profile export claude --to "$BUNDLE" 2>&1)"; rc=$?
[ "$rc" = 0 ] && ok "export writes a bundle" || { bad "export writes a bundle"; echo "$out"; }

for want in profile/CLAUDE.md profile/settings.json profile/skills/demo/SKILL.md \
            profile/commands/go.md profile/statusline.sh \
            profile/plugins/known_marketplaces.json home/.claude.json manifest.json; do
    [ -e "${BUNDLE}/${want}" ] || { bad "the bundle carries ${want}"; continue; }
done
ok "the bundle carries the reusable half"

leaked=""
for s in "${SECRETS[@]}"; do
    grep -rq -- "$s" "$BUNDLE" 2>/dev/null && leaked="${leaked} ${s}"
done
[ -z "$leaked" ] && ok "no credential, transcript or machine identity reached the bundle" \
    || bad "the bundle carries:${leaked}"

[ -x "${BUNDLE}/profile/statusline.sh" ] \
    && ok "the status line keeps its executable bit" \
    || bad "the status line keeps its executable bit"

python3 - "$BUNDLE" <<'PY' > "${WORK}/manifest.out" 2>&1
import json, os, sys
b = sys.argv[1]
doc = json.load(open(os.path.join(b, "manifest.json")))
assert doc["agent"] == "claude", doc
assert doc["version"] == 1, doc
bad = [f for f in doc["files"] if f["class"] not in ("reusable", "mixed")]
assert not bad, bad
# The names survive so the importing machine knows what to supply.
s = json.load(open(os.path.join(b, "profile/settings.json")))
assert s["env"] == {"GITHUB_TOKEN": None}, s["env"]
assert s["model"] == "opus", s
m = json.load(open(os.path.join(b, "home/.claude.json")))
assert list(m) == ["mcpServers"], list(m)
assert m["mcpServers"]["vault"]["headers"] == {"Authorization": None}, m
assert m["mcpServers"]["vault"]["url"] == "https://vault.example/mcp", m
assert "oauthAccount" not in m["mcpServers"]["vault"], m
k = json.load(open(os.path.join(b, "profile/plugins/known_marketplaces.json")))
assert "installLocation" not in k["mkt"], k
assert k["mkt"]["source"]["repo"] == "acme/mkt", k
print("ok")
PY
grep -qx ok "${WORK}/manifest.out" \
    && ok "the bundle keeps the names and drops the values" \
    || { bad "the bundle keeps the names and drops the values"; cat "${WORK}/manifest.out"; }

out="$("$APEX" agent profile export claude --to "$BUNDLE" 2>&1)"
printf '%s' "$out" | grep -q 'pass --force' \
    && ok "export refuses to write over an occupied directory" \
    || { bad "export refuses to write over an occupied directory"; echo "$out"; }

# ── sync onto another machine ────────────────────────────────────────────────
section "sync"
OTHER="${WORK}/other"
mkdir -p "${OTHER}/.claude"
printf '{"model":"sonnet","env":{"GITHUB_TOKEN":"ghp_THEIR-OWN-VALUE"}}\n' \
    > "${OTHER}/.claude/settings.json"

out="$(HOME="$OTHER" "$APEX" agent profile sync claude --from "$BUNDLE" --dry-run 2>&1)"
printf '%s' "$out" | grep -q 'would change' \
    && ok "a dry run says what it would do and does nothing" \
    || { bad "a dry run says what it would do and does nothing"; echo "$out"; }
[ ! -e "${OTHER}/.claude/CLAUDE.md" ] \
    && ok "a dry run wrote nothing" || bad "a dry run wrote nothing"

out="$(HOME="$OTHER" "$APEX" agent profile sync claude --from "$BUNDLE" 2>&1)"; rc=$?
[ "$rc" = 0 ] && ok "sync applies the bundle" || { bad "sync applies the bundle"; echo "$out"; }
[ -f "${OTHER}/.claude/skills/demo/SKILL.md" ] \
    && ok "the skills arrived" || bad "the skills arrived"
[ ! -e "${OTHER}/.claude/projects" ] && [ ! -e "${OTHER}/.claude/.credentials.json" ] \
    && ok "nothing machine-local followed them" || bad "nothing machine-local followed them"

python3 - "$OTHER" <<'PY' > "${WORK}/sync.out" 2>&1
import json, os, sys
s = json.load(open(os.path.join(sys.argv[1], ".claude/settings.json")))
assert s["model"] == "opus", s
assert s["env"]["GITHUB_TOKEN"] == "ghp_THEIR-OWN-VALUE", s
print("ok")
PY
grep -qx ok "${WORK}/sync.out" \
    && ok "the import merged and did not overwrite the local value with a blank" \
    || { bad "the import merged and did not overwrite the local value with a blank"; cat "${WORK}/sync.out"; }
printf '%s' "$out" | grep -q 'GITHUB_TOKEN' \
    && ok "the operator is told which values the bundle could not carry" \
    || { bad "the operator is told which values the bundle could not carry"; echo "$out"; }

out="$(HOME="$OTHER" "$APEX" agent profile sync claude --from "$BUNDLE" 2>&1)"
printf '%s' "$out" | grep -q 'nothing to change' \
    && ok "a second sync of the same bundle changes nothing" \
    || { bad "a second sync of the same bundle changes nothing"; echo "$out"; }

# A bundle is a file somebody sent. A hand-edited manifest must reach nothing.
HOSTILE="${WORK}/hostile"
mkdir -p "${HOSTILE}/home/.ssh"
printf 'ssh-rsa AAAA attacker\n' > "${HOSTILE}/home/.ssh/authorized_keys"
printf '{"version":1,"agent":"claude","files":[{"path":"home/.ssh/authorized_keys","class":"reusable"}]}\n' \
    > "${HOSTILE}/manifest.json"
HOME="$OTHER" "$APEX" agent profile sync claude --from "$HOSTILE" >/dev/null 2>&1
[ ! -e "${OTHER}/.ssh/authorized_keys" ] \
    && ok "a manifest naming a path outside the profile reaches nothing" \
    || bad "a manifest naming a path outside the profile reaches nothing"

# ── the mounts a confined session actually gets (P0-010) ─────────────────────
#
# A real daemon, a real bwrap namespace and a real agent process. The agent is
# a stub on PATH, because `claude` is resolved through PATH at spawn time —
# which is what lets a user's own build win, and what lets this assert the
# mount code without an account or a network.
section "the mounts a confined session gets"
STUB="${WORK}/stub"
mkdir -p "$STUB"
# The probe reports into the project directory, which is bound read-write and
# outlives the session — a transcript does not: the runtime drops a finished
# session's log, so a suite that read one would be asserting on an empty file.
#
# Every write is attempted in a subshell. A redirection failure on a POSIX
# special built-in (`:` is one) exits a non-interactive shell outright, so a
# probe written the obvious way stops at the first read-only mount and reports
# nothing at all — which reads as a session that never ran.
cat > "${STUB}/claude" <<'STUBEOF'
#!/bin/sh
# Not an agent: a probe that reports what the mount namespace let it do.
OUT="${PWD}/probe.out"
: > "$OUT"
p() { printf '%s=%s\n' "$1" "$2" >> "$OUT"; }
w() { ( printf 'probe\n' > "$2" ) >/dev/null 2>&1; p "$1" $?; }
a() { ( printf 'tampered\n' >> "$2" ) >/dev/null 2>&1; p "$1" $?; }
r() { ( cat "$2" ) >/dev/null 2>&1; p "$1" $?; }

w write_skills          "$HOME/.claude/skills/probe"
w write_commands        "$HOME/.claude/commands/probe"
w write_agents          "$HOME/.claude/agents/probe"
a append_instructions   "$HOME/.claude/CLAUDE.md"
w overwrite_settings    "$HOME/.claude/settings.json"
w overwrite_marketplace "$HOME/.claude/plugins/known_marketplaces.json"

w write_projects        "$HOME/.claude/projects/session-probe"
w write_plugin_cache    "$HOME/.claude/plugins/cache/session-probe"
w write_plugin_data     "$HOME/.claude/plugins/data/session-probe"
w write_todos           "$HOME/.claude/todos/session-probe"
w write_sidecar         "$HOME/.claude.json"
w write_unlisted        "$HOME/.claude/invented-by-a-later-release"

r read_skills           "$HOME/.claude/skills/demo/SKILL.md"
r read_instructions     "$HOME/.claude/CLAUDE.md"
r read_credentials      "$HOME/.claude/.credentials.json"
r read_ssh              "$HOME/.ssh/id_ed25519"
p done 0
STUBEOF
chmod 0755 "${STUB}/claude"
export PATH="${STUB}:${PATH}"

# `todos` is deliberately absent from the fixture: the daemon has to create the
# writable directories before the session, or bwrap's `-try` bind is a no-op
# and everything written there lands in the tmpfs that masks $HOME.
[ ! -e "${C}/todos" ] || rmdir "${C}/todos"
# A private key that must stay out of reach, so the sandbox's own default-deny
# is asserted around all of this rather than assumed.
mkdir -p "${HOME}/.ssh"; printf 'PRIVATE-SSH-KEY\n' > "${HOME}/.ssh/id_ed25519"

mkdir -p "${WORK}/proj"
"$AGENTD" > "${WORK}/agentd.log" 2>&1 &
DAEMON_PID=$!
SOCK="${XDG_RUNTIME_DIR}/apex-agentd/control.sock"
for _ in $(seq 1 50); do [ -S "$SOCK" ] && break; sleep 0.1; done
if [ -S "$SOCK" ]; then
    ok "the daemon came up on an isolated socket"
else
    bad "the daemon came up on an isolated socket"
    sed 's/^/      /' "${WORK}/agentd.log"
    printf '\nprofile: %d passed, %d failed\n' "$pass" "$fail"
    exit 1
fi

out="$("$APEX" agent run --detach --agent claude --sandbox project --network offline \
        --cwd "${WORK}/proj" 2>&1)"
id="$(printf '%s' "$out" | sed -n 's/^session \([0-9]*\) .*/\1/p' | head -1)"
if [ -n "$id" ]; then
    ok "a confined claude session started (id ${id})"
else
    bad "a confined claude session started"
    printf '%s\n' "$out" | sed 's/^/      /'
    sed 's/^/      /' "${WORK}/agentd.log"
    printf '\nprofile: %d passed, %d failed\n' "$pass" "$fail"
    exit 1
fi

LOG="${WORK}/proj/probe.out"
for _ in $(seq 1 150); do
    grep -q '^done=' "$LOG" 2>/dev/null && break
    sleep 0.1
done
if ! grep -q '^done=' "$LOG" 2>/dev/null; then
    bad "the session ran to completion"
    cat "$LOG" 2>/dev/null | sed 's/^/      /'
    sed 's/^/      /' "${WORK}/agentd.log"
    printf '\nprofile: %d passed, %d failed\n' "$pass" "$fail"
    exit 1
fi
ok "the session ran to completion"
probe() { grep -o "^$1=[0-9]*" "$LOG" | tail -1 | cut -d= -f2; }

# Criterion 1: config, skills and commands are mounted read-only.
refused=""
for name in write_skills write_commands write_agents append_instructions \
            overwrite_settings overwrite_marketplace; do
    [ "$(probe "$name")" = "0" ] && refused="${refused} ${name}"
done
[ -z "$refused" ] && ok "config, skills, commands and agents are read-only to the session" \
    || { bad "the session could write:${refused}"; cat "$LOG"; }
[ "$(probe read_skills)" = "0" ] && [ "$(probe read_instructions)" = "0" ] \
    && ok "and still readable" || { bad "and still readable"; cat "$LOG"; }
grep -q tampered "${C}/CLAUDE.md" \
    && bad "the session rewrote the instructions it was started with" \
    || ok "the instructions on disk are untouched"

# Criterion 2: writable session and plugin state, isolated from the rest.
denied=""
for name in write_projects write_plugin_cache write_plugin_data write_todos \
            write_sidecar write_unlisted; do
    [ "$(probe "$name")" = "0" ] || denied="${denied} ${name}"
done
[ -z "$denied" ] && ok "session and plugin state is writable" \
    || { bad "the session could not write:${denied}"; cat "$LOG"; }
[ -e "${C}/projects/session-probe" ] && [ -e "${C}/plugins/cache/session-probe" ] \
    && ok "what the session wrote to its session and plugin state persisted" \
    || bad "what the session wrote to its session and plugin state persisted"
[ -d "${C}/todos" ] && [ -e "${C}/todos/session-probe" ] \
    && ok "a missing writable directory was created before the session" \
    || bad "a missing writable directory was created before the session"
[ ! -e "${C}/invented-by-a-later-release" ] \
    && ok "a path the table does not name stayed in the session's own overlay" \
    || bad "a path the table does not name stayed in the session's own overlay"

# The sandbox's own default-deny still holds around all of it.
[ "$(probe read_credentials)" = "0" ] \
    && ok "the agent still reaches its own credentials" \
    || { bad "the agent still reaches its own credentials"; cat "$LOG"; }
[ "$(probe read_ssh)" != "0" ] \
    && ok "the rest of the home is still not there" \
    || { bad "the rest of the home is still not there"; cat "$LOG"; }

printf '\nprofile: %d passed, %d failed\n' "$pass" "$fail"
[ "$fail" = 0 ]
