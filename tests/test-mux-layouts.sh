#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  Terminal layout templates, in a real tmux and a real zellij.
#
#  The unit tests in apex-agent-core cover which pane runs what, the editor and
#  backend choice, and the wire format. What they cannot cover is the half that
#  only a running multiplexer can answer: that the panes come out in the right
#  ORDER with the right titles, that reopening attaches instead of rebuilding,
#  and that zellij accepts the layout this generates.
#
#  So every assertion below drives a real `tmux` or `zellij`, on an ISOLATED
#  socket — TMUX_TMPDIR and ZELLIJ_SOCKET_DIR both point under $WORK. The
#  developer's own sessions are never listed, never attached to and never
#  killed.
#
#  Nothing here reaches the agent runtime either. XDG_RUNTIME_DIR is a fixture
#  with no control socket, so `apex agent list` finds no daemon, and `apex` on
#  PATH inside the panes is a stub. An earlier version of this file did not do
#  the second, and a pane genuinely started a claude session on the developer's
#  own daemon.
#
#      ./tests/test-mux-layouts.sh
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
# Counted failures, not `set -e`: several assertions run commands that exit
# non-zero on purpose, and under `bash -e {0}` on CI an assignment from one of
# them kills the whole script part-way through.
set +e

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"

pass=0; fail=0; skip=0; sections_run=0
ok()   { printf 'PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad()  { printf 'FAIL  %s\n' "$1"; fail=$((fail + 1)); }
skipped() { printf 'SKIP  %s — %s\n' "$1" "$2"; skip=$((skip + 1)); }
section() { printf '\n── %s ──\n' "$1"; }

for tool in cargo git; do
    command -v "$tool" >/dev/null 2>&1 || {
        echo "FATAL: $tool is required; this suite cannot test anything without it" >&2
        exit 2
    }
done

MUX="${ROOT}/files/system/libexec/apex-mux"
[ -x "$MUX" ] || { echo "FATAL: ${MUX} is missing or not executable" >&2; exit 2; }

# ── isolation ────────────────────────────────────────────────────────────────
export TMUX_TMPDIR="${WORK}/tmux"
export ZELLIJ_SOCKET_DIR="${WORK}/zellij-sock"
export XDG_CONFIG_HOME="${WORK}/config"
export XDG_DATA_HOME="${WORK}/data"
export XDG_CACHE_HOME="${WORK}/cache"
export XDG_STATE_HOME="${WORK}/state"
# No control socket lives here, so the CLI finds no agent runtime and every
# agent pane resolves to `apex agent run`. Deterministic, and it means this
# suite cannot start or attach to a session on the developer's daemon.
export XDG_RUNTIME_DIR="${WORK}/run"
mkdir -p "$TMUX_TMPDIR" "$ZELLIJ_SOCKET_DIR" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" \
         "$XDG_CACHE_HOME" "$XDG_STATE_HOME" "$XDG_RUNTIME_DIR"
unset TMUX ZELLIJ

cleanup() {
    if command -v tmux >/dev/null 2>&1; then
        tmux kill-server >/dev/null 2>&1
    fi
    if command -v zellij >/dev/null 2>&1; then
        zellij kill-all-sessions -y >/dev/null 2>&1
        zellij delete-all-sessions -y >/dev/null 2>&1
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT

# A stub `apex` for anything a PANE runs. The panes are real processes; without
# this the agent panes would call the real CLI and reach the real daemon.
BIN="${WORK}/bin"; mkdir -p "$BIN"
cat > "${BIN}/apex" <<'EOF'
#!/bin/sh
echo "stub apex: $*"
sleep 600
EOF
chmod +x "${BIN}/apex"
# A predictable editor, so the assertions do not depend on what is installed.
cat > "${BIN}/apexed" <<'EOF'
#!/bin/sh
sleep 600
EOF
chmod +x "${BIN}/apexed"
export PATH="${BIN}:${PATH}"
export VISUAL=apexed

PROJ="${WORK}/demo"
mkdir -p "$PROJ"
git -C "$PROJ" init -q 2>/dev/null
git -C "$PROJ" -c user.email=t@t -c user.name=t commit -q --allow-empty -m init 2>/dev/null

PLAN="${WORK}/plan.tsv"
printf 'editor\t%s\tapexed\nagent\t%s\tapex\tagent\trun\nterminal\t%s\n' \
    "$PROJ" "$PROJ" "$PROJ" > "$PLAN"

# ─────────────────────────────────────────────────────────────────────────────
section "the adapter refuses what it does not understand"
sections_run=$((sections_run + 1))

sh -n "$MUX" && ok "apex-mux parses" || bad "apex-mux parses"

out="$("$MUX" nonsense 2>&1)"; rc=$?
[ "$rc" -eq 2 ] && printf '%s' "$out" | grep -q 'usage:' \
    && ok "an unknown verb is a usage error" || bad "an unknown verb is a usage error"

out="$("$MUX" has screen sess 2>&1)"; rc=$?
[ "$rc" -ne 0 ] && printf '%s' "$out" | grep -q 'not a multiplexer' \
    && ok "a multiplexer it does not drive is named as such" \
    || bad "a multiplexer it does not drive is named as such"

out="$("$MUX" build tmux sess sideways "$PLAN" 2>&1)"; rc=$?
[ "$rc" -ne 0 ] && printf '%s' "$out" | grep -q 'not an arrangement' \
    && ok "an arrangement it does not know is refused" \
    || bad "an arrangement it does not know is refused"

out="$("$MUX" build tmux sess tiled "${WORK}/no-such-plan" 2>&1)"; rc=$?
[ "$rc" -ne 0 ] && printf '%s' "$out" | grep -q 'cannot read the plan' \
    && ok "a missing plan file is reported, not treated as empty" \
    || bad "a missing plan file is reported, not treated as empty"

"$MUX" backends | grep -qE '^(tmux|zellij)$' \
    && ok "backends lists what is installed" || bad "backends lists what is installed"

# ─────────────────────────────────────────────────────────────────────────────
section "tmux"
if ! command -v tmux >/dev/null 2>&1; then
    skipped "tmux integration" "tmux is not installed on this machine"
else
    sections_run=$((sections_run + 1))
    S="apex-demo-test"

    "$MUX" has tmux "$S" 2>/dev/null \
        && bad "a session that does not exist is reported as absent" \
        || ok "a session that does not exist is reported as absent"

    "$MUX" build tmux "$S" main-vertical "$PLAN" \
        && ok "build creates the session" || bad "build creates the session"

    "$MUX" has tmux "$S" \
        && ok "and then reports it as present" || bad "and then reports it as present"

    # Order, titles, commands and cwd together. The order is the assertion that
    # caught a real defect: `split-window` renumbers the panes it pushes along,
    # so addressing panes by index put every title one place out.
    got="$(tmux list-panes -t "$S:0" -F '#{pane_index}:#{pane_title}:#{pane_start_command}' | tr '\n' ' ')"
    [ "$got" = "0:editor:apexed 1:agent:apex agent run 2:terminal: " ] \
        && ok "the panes are in template order with their own commands" \
        || { bad "the panes are in template order with their own commands"; printf '      %s\n' "$got"; }

    got="$(tmux list-panes -t "$S:0" -F '#{pane_current_path}' | sort -u)"
    [ "$got" = "$PROJ" ] \
        && ok "every pane starts in the project" \
        || { bad "every pane starts in the project"; printf '      %s\n' "$got"; }

    # A pane whose command finishes must not close: the layout's whole value is
    # that the shape stays where it was put.
    tmux show-options -t "$S:0" remain-on-exit 2>/dev/null | grep -q 'on' \
        && ok "a finished command leaves the pane in place" \
        || bad "a finished command leaves the pane in place"

    # main-vertical means one full-height pane on the left. tmux's layout string
    # describes the geometry, so this is checked against the geometry rather
    # than against the command that was issued.
    lay="$(tmux display-message -p -t "$S:0" '#{window_layout}')"
    printf '%s' "$lay" | grep -q '\[' \
        && ok "the arrangement really splits the window" \
        || { bad "the arrangement really splits the window"; printf '      %s\n' "$lay"; }

    before="$(tmux list-panes -t "$S:0" | wc -l)"
    "$MUX" build tmux "$S" main-vertical "$PLAN"
    after="$(tmux list-panes -t "$S:0" | wc -l)"
    [ "$before" = "$after" ] && [ "$after" = "3" ] \
        && ok "building an open session does not rebuild it" \
        || bad "building an open session does not rebuild it (${before} -> ${after})"

    # "restore cleanly": the multiplexer is gone, the template rebuilds it.
    tmux kill-session -t "=$S" >/dev/null 2>&1
    "$MUX" has tmux "$S" 2>/dev/null && bad "killing the session removes it" \
                                     || ok "killing the session removes it"
    "$MUX" build tmux "$S" main-vertical "$PLAN" >/dev/null 2>&1
    got="$(tmux list-panes -t "$S:0" -F '#{pane_title}' | tr '\n' ' ')"
    [ "$got" = "editor agent terminal " ] \
        && ok "the same template rebuilds the same panes afterwards" \
        || { bad "the same template rebuilds the same panes afterwards"; printf '      %s\n' "$got"; }

    # A pane command is passed as argv and never through a shell.
    SHPLAN="${WORK}/shellish.tsv"
    printf 'trick\t%s\tapexed\t;\ttouch\t%s/pwned\n' "$PROJ" "$WORK" > "$SHPLAN"
    tmux kill-session -t "=$S" >/dev/null 2>&1
    "$MUX" build tmux "apex-shellish" tiled "$SHPLAN" >/dev/null 2>&1
    sleep 0.5
    [ ! -e "${WORK}/pwned" ] \
        && ok "a pane command is argv, never a shell string" \
        || bad "a pane command is argv, never a shell string"
    tmux kill-server >/dev/null 2>&1
fi

# ─────────────────────────────────────────────────────────────────────────────
section "zellij"
if ! command -v zellij >/dev/null 2>&1; then
    skipped "zellij integration" "zellij is not installed on this machine"
else
    sections_run=$((sections_run + 1))
    Z="apex-demo-zellij"

    "$MUX" has zellij "$Z" 2>/dev/null \
        && bad "a zellij session that does not exist is reported as absent" \
        || ok "a zellij session that does not exist is reported as absent"

    timeout 90 "$MUX" build zellij "$Z" main-vertical "$PLAN" \
        && ok "build creates the zellij session" || bad "build creates the zellij session"
    sleep 2

    "$MUX" has zellij "$Z" \
        && ok "and then reports it as present" || bad "and then reports it as present"

    dump="$(timeout 30 zellij --session "$Z" action dump-layout 2>&1)"
    printf '%s' "$dump" | grep -q 'tab name="apex"' \
        && ok "the layout landed as a tab zellij can describe" \
        || { bad "the layout landed as a tab zellij can describe"; printf '%s' "$dump" | head -5 | sed 's/^/      /'; }

    # The commands are genuinely running, not declared and suspended. `apexed`
    # is the fixture editor, so a live process with that name is the proof.
    pgrep -f 'apexed' >/dev/null 2>&1 \
        && ok "the pane commands are actually running" \
        || bad "the pane commands are actually running"

    tabs_before="$(printf '%s' "$dump" | grep -c 'tab name=')"
    timeout 90 "$MUX" build zellij "$Z" main-vertical "$PLAN" >/dev/null 2>&1
    sleep 1
    tabs_after="$(timeout 30 zellij --session "$Z" action dump-layout 2>&1 | grep -c 'tab name=')"
    [ "$tabs_before" = "$tabs_after" ] \
        && ok "building an open zellij session adds nothing" \
        || bad "building an open zellij session adds nothing (${tabs_before} -> ${tabs_after})"

    zellij kill-session "$Z" >/dev/null 2>&1
    zellij delete-session "$Z" >/dev/null 2>&1
    sleep 1

    # Both arrangements have to be layouts zellij ACCEPTS. A layout it cannot
    # parse fails at open time with a KDL error and no session, which is the one
    # failure mode a generated format has — so it is checked directly, against
    # zellij's own parser, with no terminal needed: a bad layout is rejected
    # before zellij ever tries to take over the tty.
    for arr in main-vertical tiled; do
        kdl="$("$MUX" kdl "$arr" "$PLAN")"
        printf '%s' "$kdl" | grep -q 'tab name="apex"' \
            && ok "the ${arr} layout is generated" || bad "the ${arr} layout is generated"
        # Handed straight back to zellij's own parser. No terminal is needed:
        # zellij parses the layout before it tries to take over the tty, so a
        # bad one says "Failed to parse" and a good one gets as far as the
        # terminal it does not have.
        out="$(timeout 30 zellij --layout-string "$kdl" 2>&1 </dev/null)"
        printf '%s' "$out" | grep -q 'Failed to parse' \
            && { bad "zellij accepts the ${arr} layout"; printf '%s' "$out" | head -4 | sed 's/^/      /'; } \
            || ok "zellij accepts the ${arr} layout"
    done
    zellij kill-all-sessions -y >/dev/null 2>&1
    zellij delete-all-sessions -y >/dev/null 2>&1
fi

# ─────────────────────────────────────────────────────────────────────────────
section "apex project layout templates / open"
if ! cargo build --manifest-path "${ROOT}/apexd/Cargo.toml" --bin apex >/dev/null 2>&1; then
    bad "apex builds"
else
    sections_run=$((sections_run + 1))
    ok "apex builds"
    APEX="${CARGO_TARGET_DIR:-${ROOT}/apexd/target}/debug/apex"
    export APEX_MUX_ADAPTER="$MUX"

    out="$(cd "$PROJ" && "$APEX" project layout templates 2>&1)"
    for t in dev review agents; do
        printf '%s' "$out" | grep -q "^${t} " \
            && ok "the ${t} template is listed" || bad "the ${t} template is listed"
    done

    out="$(cd "$PROJ" && "$APEX" project layout open --mux tmux --dry-run 2>&1)"
    printf '%s' "$out" | grep -q 'editor *apexed' \
        && ok "dev opens the editor from \$VISUAL" \
        || { bad "dev opens the editor from \$VISUAL"; printf '      %s\n' "$out"; }
    printf '%s' "$out" | grep -q 'agent *apex agent run' \
        && ok "with no runtime reachable, the agent pane starts a session" \
        || bad "with no runtime reachable, the agent pane starts a session"
    printf '%s' "$out" | grep -q 'terminal *<shell>' \
        && ok "the terminal pane runs the shell rather than a named one" \
        || bad "the terminal pane runs the shell rather than a named one"

    out="$(cd "$PROJ" && "$APEX" project layout open review --mux tmux --dry-run 2>&1)"
    printf '%s' "$out" | grep -q 'diff *apex agent diff' \
        && ok "review points its third pane at the diff" || bad "review points its third pane at the diff"

    out="$(cd "$PROJ" && "$APEX" project layout open agents --mux tmux --agents 3 --dry-run 2>&1)"
    [ "$(printf '%s' "$out" | grep -c 'agent  *apex agent run')" = "3" ] \
        && ok "the multi-agent template opens the agents asked for" \
        || { bad "the multi-agent template opens the agents asked for"; printf '      %s\n' "$out"; }

    out="$(cd "$PROJ" && "$APEX" project layout open agents --mux tmux --agents 400 --dry-run 2>&1)"
    [ "$(printf '%s' "$out" | grep -c 'agent  *apex agent run')" = "8" ] \
        && ok "the agent count is bounded rather than obeyed" \
        || bad "the agent count is bounded rather than obeyed"

    out="$(cd "$PROJ" && "$APEX" project layout open nope --mux tmux --dry-run 2>&1)"
    printf '%s' "$out" | grep -q 'no template called nope' \
        && ok "an unknown template names the command that lists them" \
        || bad "an unknown template names the command that lists them"

    out="$(cd "$PROJ" && "$APEX" project layout open --mux screen --dry-run 2>&1)"
    printf '%s' "$out" | grep -q 'not a multiplexer' \
        && ok "a multiplexer APEX does not drive is refused" \
        || bad "a multiplexer APEX does not drive is refused"

    out="$(cd "${WORK}" && "$APEX" project layout open --dry-run 2>&1)"
    printf '%s' "$out" | grep -q 'not inside a git repository' \
        && ok "outside a project it says so" || bad "outside a project it says so"

    # A dry run starts nothing at all.
    if command -v tmux >/dev/null 2>&1; then
        tmux kill-server >/dev/null 2>&1
        (cd "$PROJ" && "$APEX" project layout open --mux tmux --dry-run >/dev/null 2>&1)
        tmux list-sessions >/dev/null 2>&1 \
            && bad "a dry run starts no multiplexer session" \
            || ok "a dry run starts no multiplexer session"

        # The real thing. `open` ends by handing the terminal to tmux, which has
        # no terminal here — so the attach fails and the BUILD is what is
        # asserted, which is the part this owns.
        (cd "$PROJ" && "$APEX" project layout open --mux tmux </dev/null >/dev/null 2>&1)
        name="$(tmux list-sessions -F '#{session_name}' 2>/dev/null | head -1)"
        case "$name" in
            apex-demo-*) ok "the session is named after the project" ;;
            *) bad "the session is named after the project (got '${name}')" ;;
        esac
        got="$(tmux list-panes -a -F '#{pane_title}' 2>/dev/null | tr '\n' ' ')"
        [ "$got" = "editor agent terminal " ] \
            && ok "open builds the template's panes" \
            || { bad "open builds the template's panes"; printf '      %s\n' "$got"; }

        # The template is remembered on the project's ONE layout record, so
        # reopening needs no argument — and `layout show` reports it.
        out="$(cd "$PROJ" && "$APEX" project layout show 2>&1)"
        printf '%s' "$out" | grep -q 'terminal template: dev in tmux' \
            && ok "the template is remembered on the project's layout record" \
            || { bad "the template is remembered on the project's layout record"; printf '      %s\n' "$out"; }

        out="$(cd "$PROJ" && "$APEX" project layout open </dev/null 2>&1)"
        printf '%s' "$out" | grep -q 'is already open — attaching' \
            && ok "reopening attaches instead of rebuilding" \
            || { bad "reopening attaches instead of rebuilding"; printf '      %s\n' "$out"; }

        # A record with a template and no captured windows is not "restore
        # nothing and call it success".
        out="$(cd "$PROJ" && "$APEX" project layout restore --dry-run 2>&1)"
        printf '%s' "$out" | grep -q 'no windows are saved' \
            && ok "restoring a template-only record explains what is missing" \
            || { bad "restoring a template-only record explains what is missing"; printf '      %s\n' "$out"; }

        tmux kill-server >/dev/null 2>&1
    else
        skipped "the open/attach assertions" "tmux is not installed on this machine"
    fi
fi

# ─────────────────────────────────────────────────────────────────────────────
printf '\nmux-layouts: %d passed, %d failed, %d skipped\n' "$pass" "$fail" "$skip"
if [ "$sections_run" -eq 0 ]; then
    echo "FATAL: every section skipped — this run asserted nothing" >&2
    exit 2
fi
[ "$fail" -eq 0 ]
