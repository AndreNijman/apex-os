#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  End-to-end assertions for `apex project layout` (roadmap §6).
#
#  The unit tests in apex-agent-core cover the capture rule, the descendant cwd
#  walk, the terminal-flag table and the store. What they cannot cover is the
#  command: that `save` refuses to overwrite a good layout with an empty one,
#  that `restore` never involves a shell, and that a window belonging to a
#  DIFFERENT project is not captured.
#
#  The compositor adapter is FAKED, through APEX_WINDOW_ADAPTER. That is the
#  whole reason the adapter is a separate program: the window list is the only
#  compositor-specific input, so replacing it makes every assertion below
#  deterministic and runnable with no compositor at all.
#
#  Nothing here starts a real window. `restore` is only ever called with
#  --dry-run, so the suite cannot open anything on the developer's desktop.
#
#      ./tests/test-project-layout.sh
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

pass=0; fail=0
ok()  { printf 'PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL  %s\n' "$1"; fail=$((fail + 1)); }
section() { printf '\n── %s ──\n' "$1"; }

section "the binary"
if ! command -v cargo >/dev/null 2>&1; then
    printf 'SKIP  cargo unavailable\n\nproject-layout: 0 passed, 0 failed (skipped)\n'
    exit 0
fi
if ! cargo build --manifest-path "${ROOT}/apexd/Cargo.toml" --bin apex >/dev/null 2>&1; then
    bad "apex builds"
    printf '\nproject-layout: %d passed, %d failed\n' "$pass" "$fail"
    exit 1
fi
ok "apex builds"
APEX="${ROOT}/apexd/target/debug/apex"

# Isolated state, so the developer's own saved layouts are never touched.
export XDG_STATE_HOME="${WORK}/state"
mkdir -p "$XDG_STATE_HOME"

# Two projects, so "belongs to this project" can actually be tested.
PROJ="${WORK}/mine"
OTHER="${WORK}/theirs"
for d in "$PROJ" "$OTHER"; do
    mkdir -p "$d"
    git -C "$d" init -q 2>/dev/null
    git -C "$d" -c user.email=t@t -c user.name=t commit -q --allow-empty -m init 2>/dev/null
done

# ── the fake adapter ─────────────────────────────────────────────────────────
# Emits whatever ${WORK}/windows.json holds. The pids in it are REAL processes
# this script starts, because the capture rule reads /proc — a fabricated pid
# would be skipped as dead and every assertion would pass for the wrong reason.
FAKE="${WORK}/fake-adapter"
cat > "$FAKE" <<EOF
#!/bin/sh
case "\$1" in
    list)       cat "${WORK}/windows.json" ;;
    compositor) echo fake ;;
    *)          echo "fake adapter: \$1 unsupported" >&2; exit 1 ;;
esac
EOF
chmod +x "$FAKE"
export APEX_WINDOW_ADAPTER="$FAKE"

# Long-lived processes whose cwd is each project. `sh -c 'cd X; sleep'` is not
# enough: the capture reads /proc/<pid>/cwd, so the process must genuinely BE
# in that directory.
( cd "$PROJ"  && exec sleep 600 ) & MINE_PID=$!
( cd "$OTHER" && exec sleep 600 ) & THEIRS_PID=$!
trap 'kill "$MINE_PID" "$THEIRS_PID" 2>/dev/null; rm -rf "$WORK"' EXIT
sleep 0.3

windows() { printf '%s' "$1" > "${WORK}/windows.json"; }

# ── nothing to save ──────────────────────────────────────────────────────────
section "an empty capture does not destroy a good layout"
windows '[]'
out="$(cd "$PROJ" && "$APEX" project layout save 2>&1)"
printf '%s' "$out" | grep -q "nothing saved" \
    && ok "an empty window list saves nothing and says so" \
    || { bad "an empty window list saves nothing and says so"; printf '      %s\n' "$out"; }

# ── capture ──────────────────────────────────────────────────────────────────
section "only windows working inside the project are captured"
windows "$(cat <<EOF
[
  { "handle": "0xdeadbeef", "pid": ${MINE_PID},   "app_id": "Alacritty", "title": "mine",   "workspace": "2", "floating": false },
  { "handle": "0xcafebabe", "pid": ${THEIRS_PID}, "app_id": "Alacritty", "title": "theirs", "workspace": "5", "floating": false },
  { "handle": null,         "pid": null,          "app_id": "labwcwin",  "title": "no pid", "workspace": "",  "floating": null }
]
EOF
)"
out="$(cd "$PROJ" && "$APEX" project layout save 2>&1)"
printf '%s' "$out" | grep -q "saved 1 window" \
    && ok "exactly one window is captured" \
    || { bad "exactly one window is captured"; printf '      %s\n' "$out"; }
printf '%s' "$out" | grep -q "workspace(s) 2" \
    && ok "the workspace is recorded" || bad "the workspace is recorded"

json="$(cd "$PROJ" && "$APEX" project layout show --json 2>/dev/null)"
printf '%s' "$json" | python3 -c "
import json,sys
l = json.load(sys.stdin)
e = l['entries']
assert len(e) == 1, e
assert e[0]['cwd'] == '${PROJ}', e
assert e[0]['workspace'] == '2', e
assert e[0]['terminal'] is True, e
assert l['saved'] > 0, l
" 2>"${WORK}/cap.err" \
    && ok "the entry records the project's own cwd" \
    || { bad "the entry records the project's own cwd"; sed 's/^/      /' "${WORK}/cap.err"; }

# The property that makes a layout restorable at all.
printf '%s' "$json" | grep -q "0xdeadbeef" \
    && bad "no compositor window handle is stored" \
    || ok "no compositor window handle is stored"

section "the other project's layout is its own"
out="$(cd "$OTHER" && "$APEX" project layout save 2>&1)"
printf '%s' "$out" | grep -q "saved 1 window" \
    && ok "the other project captures its own window" || bad "the other project captures its own window"
mine="$(cd "$PROJ" && "$APEX" project layout show --json 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["entries"][0]["cwd"])')"
[ "$mine" = "$PROJ" ] \
    && ok "saving one project's layout did not overwrite the other's" \
    || bad "saving one project's layout did not overwrite the other's (got ${mine})"

# ── restore ──────────────────────────────────────────────────────────────────
section "restore rebuilds a terminal with its directory"
# The stored argv for the fixture is `sleep 600`, not a terminal — but it was
# classified as one from its app_id, which is exactly the case that matters:
# a terminal's stored argv does NOT carry its working directory, so replaying
# it verbatim opens a terminal in the wrong place.
out="$(cd "$PROJ" && TERMINAL=foot "$APEX" project layout restore --dry-run 2>&1)"
printf '%s\n' "$out" | sed 's/^/      /'
if command -v foot >/dev/null 2>&1; then
    printf '%s' "$out" | grep -q -- "--working-directory ${PROJ}" \
        && ok "a terminal is restored with the project directory" \
        || bad "a terminal is restored with the project directory"
else
    # foot is not installed, so choose_terminal falls through to whatever is —
    # the assertion is that SOME emulator plus the directory is emitted, not
    # which one.
    printf '%s' "$out" | grep -qF "${PROJ}" \
        && ok "a terminal is restored with the project directory" \
        || bad "a terminal is restored with the project directory"
fi
printf '%s' "$out" | grep -q "1 window(s) would be restored" \
    && ok "the dry run reports what it would do" || bad "the dry run reports what it would do"

section "a dry run starts nothing"
before="$(pgrep -c -x sleep 2>/dev/null || echo 0)"
(cd "$PROJ" && "$APEX" project layout restore --dry-run >/dev/null 2>&1)
sleep 0.5
after="$(pgrep -c -x sleep 2>/dev/null || echo 0)"
[ "$before" = "$after" ] \
    && ok "no process was started by a dry run" \
    || bad "a dry run started something (${before} -> ${after})"

section "an application is restored verbatim"
# A non-terminal app_id means the stored argv IS the answer — it carries its
# own arguments, and substituting a terminal command would lose them.
windows "$(cat <<EOF
[ { "handle": null, "pid": ${MINE_PID}, "app_id": "firefox", "title": "app", "workspace": "3", "floating": false } ]
EOF
)"
(cd "$PROJ" && "$APEX" project layout save >/dev/null 2>&1)
out="$(cd "$PROJ" && "$APEX" project layout restore --dry-run 2>&1)"
printf '%s' "$out" | grep -q "would run (ws 3): sleep 600" \
    && ok "an application keeps its own argv" \
    || { bad "an application keeps its own argv"; printf '      %s\n' "$out"; }

section "forget"
# Output is captured and THEN matched, never piped straight into grep. Under
# `set -o pipefail` a command that exits non-zero fails the whole pipeline even
# when grep matched — and every assertion below is about a command that
# correctly exits non-zero, so piping made two of them fail for a reason that
# had nothing to do with the behaviour being tested.
(cd "$PROJ" && "$APEX" project layout forget >/dev/null 2>&1)
out="$(cd "$PROJ" && "$APEX" project layout show 2>&1)"
printf '%s' "$out" | grep -q "no layout saved" \
    && ok "a forgotten layout is gone" || bad "a forgotten layout is gone"

# NOT --dry-run: with no layout there is nothing to start, and this asserts
# that the real command says so rather than doing something odd.
out="$(cd "$PROJ" && "$APEX" project layout restore 2>&1)"
printf '%s' "$out" | grep -q "no layout saved" \
    && ok "restoring nothing says so instead of failing oddly" \
    || { bad "restoring nothing says so instead of failing oddly"; printf '      %s\n' "$out"; }

section "outside a repository"
mkdir -p "${WORK}/bare"
out="$(cd "${WORK}/bare" && "$APEX" project layout save 2>&1)"
printf '%s' "$out" | grep -q "not inside a git repository" \
    && ok "a directory that is not a project is refused clearly" \
    || { bad "a directory that is not a project is refused clearly"; printf '      %s\n' "$out"; }

section "an adapter that cannot enumerate"
BROKEN="${WORK}/broken-adapter"
printf '#!/bin/sh\necho "no window query for labwc" >&2\nexit 1\n' > "$BROKEN"
chmod +x "$BROKEN"
out="$(cd "$PROJ" && APEX_WINDOW_ADAPTER="$BROKEN" "$APEX" project layout save 2>&1)"
printf '%s' "$out" | grep -q "no window query" \
    && ok "a compositor with no window query is reported, not guessed around" \
    || { bad "a compositor with no window query is reported, not guessed around"; printf '      %s\n' "$out"; }

printf '\nproject-layout: %d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
