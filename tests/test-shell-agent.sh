#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  The agent shell integration, in the shells that are not POSIX.
#
#  bash and zsh share files/desktop/shell/agent.sh. fish and nushell cannot:
#  neither can source a POSIX script, so each has its own file, and the only way
#  to know those files behave the same is to RUN each shell and compare.
#
#  So every assertion below launches a real `fish` or `nu`. Nothing here greps
#  the source for a function name and calls that a passing test — the repo has
#  been bitten by exactly that shape before.
#
#  Nothing touches the developer's own configuration. Each shell is pointed at a
#  fixture XDG_DATA_HOME / XDG_CONFIG_HOME / XDG_STATE_HOME under $WORK, and
#  `apex` on PATH is a stub that records what it was asked. The real daemon is
#  never contacted and no session is ever started.
#
#      ./tests/test-shell-agent.sh
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
# `set +e` is deliberate. This suite COUNTS failures rather than aborting, and
# several assertions run commands that exit non-zero on purpose — a guard
# firing, a usage error, a completion with the runtime down. GitHub Actions
# invokes a script as `bash -e {0}`, and under `-e` an assignment whose command
# exits non-zero kills the whole script part-way through, reporting every
# remaining assertion as a failure.
set +e

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

pass=0; fail=0; skip=0; sections_run=0
ok()   { printf 'PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad()  { printf 'FAIL  %s\n' "$1"; fail=$((fail + 1)); }
# A skip is LOUD and counted. A silent skip is how a suite reports a green tick
# over nothing asserted.
skipped() { printf 'SKIP  %s — %s\n' "$1" "$2"; skip=$((skip + 1)); }
section() { printf '\n── %s ──\n' "$1"; }

for tool in python3; do
    command -v "$tool" >/dev/null 2>&1 || {
        echo "FATAL: $tool is required; this suite cannot test anything without it" >&2
        exit 2
    }
done

FISH_CONF="${ROOT}/files/desktop/fish/apex-agent.fish"
FISH_COMP="${ROOT}/files/desktop/fish/completions"
NU_FILE="${ROOT}/files/desktop/nushell/apex.nu"

# ── the fixture ──────────────────────────────────────────────────────────────
#
# A stub `apex` that answers the four queries the integrations make, and — the
# point of it — APPENDS EVERY INVOCATION to a log. That log is what makes the
# "the prompt indicator forks nothing" assertion real rather than a claim about
# the source.
BIN="${WORK}/bin"; mkdir -p "$BIN"
CALLS="${WORK}/apex-calls.log"
: > "$CALLS"
cat > "${BIN}/apex" <<EOF
#!/bin/sh
printf '%s\n' "\$*" >> "${CALLS}"
case "\$*" in
    "agent list --json")       printf '[\n  {\n    "id": 4\n  }\n]\n' ;;
    "agent list --all --json") printf '[\n  {\n    "id": 4\n  },\n  {\n    "id": 7\n  }\n]\n' ;;
    "agent adapters")          printf 'NAME     STATUS\nclaude*  installed\ncodex    installed\n' ;;
    "secret list --json")      printf '[\n  {\n    "service": "github"\n  },\n  {\n    "service": "openai"\n  }\n]\n' ;;
    "secret capabilities")     printf 'capabilities:\n  repo.read\n  repo.write\n' ;;
    "request verbs")           printf 'verbs:\n  pkg.install\n  service.restart\n' ;;
    "request list --all --json") printf '[\n  {\n    "id": 1\n  }\n]\n' ;;
    *) printf 'STUB %s\n' "\$*" ;;
esac
EOF
chmod +x "${BIN}/apex"

# A second stub for the case that matters most in practice: the runtime is not
# running, so every query fails. Completion must stay silent.
DOWN="${WORK}/down"; mkdir -p "$DOWN"
cat > "${DOWN}/apex" <<'EOF'
#!/bin/sh
echo "apex: the agent runtime is not running" >&2
exit 1
EOF
chmod +x "${DOWN}/apex"

# A PATH with no `apex` at all — a partial image, a container. Symlinks rather
# than the real /usr/bin, because that is where `apex` lives on a developer
# machine and including it would test nothing.
NOAPEX="${WORK}/noapex"; mkdir -p "$NOAPEX"
for b in fish nu bash sh sed cat tail printf date env; do
    p="$(command -v "$b" 2>/dev/null)" && ln -sf "$p" "${NOAPEX}/${b}"
done

PROJ="${WORK}/proj"; mkdir -p "$PROJ"
OUTSIDE="${WORK}/outside"; mkdir -p "$OUTSIDE"

# Session records in the daemon's own on-disk shape: pretty-printed JSON, two
# space indent. The prompt parsers read these directly, so a fixture in any
# other shape would test a format nothing writes.
STATE="${WORK}/state"
SESS="${STATE}/apex/agent/sessions"
mkdir -p "$SESS"
record() { # id project state exit_code
    cat > "${SESS}/$1.json" <<EOF
{
  "id": $1,
  "agent": "claude",
  "cwd": "$2",
  "project": "$2",
  "project_name": "proj",
  "state": "$3",
  "exit_code": $4,
  "attached": 0
}
EOF
}
record 4 "$PROJ"    working          null
record 5 "$PROJ"    permission_request null
record 6 "$PROJ"    waiting_for_user null
record 7 "$OUTSIDE" working          null
record 8 "$PROJ"    working          0

# An empty state directory, for "nothing running".
EMPTY="${WORK}/state-empty"; mkdir -p "${EMPTY}/apex/agent/sessions"

# ── what bash says, to compare against ───────────────────────────────────────
# The parity target is not a description of the prompt format, it is the bytes
# `agent.sh` produces for the same records. Captured once, here.
bash_prompt() { # cwd state_home
    (cd "$1" && env -i PATH="${BIN}:/usr/bin:/bin" HOME="${WORK}/home" \
        XDG_STATE_HOME="$2" bash --noprofile --norc -c \
        ". '${ROOT}/files/desktop/shell/agent.sh'; apex_agent_prompt" 2>/dev/null)
}

# ─────────────────────────────────────────────────────────────────────────────
#  fish
# ─────────────────────────────────────────────────────────────────────────────
section "fish"
if ! command -v fish >/dev/null 2>&1; then
    skipped "fish integration" "fish is not installed on this machine"
elif [ ! -r "$FISH_CONF" ]; then
    bad "fish: ${FISH_CONF} exists"
else
    sections_run=$((sections_run + 1))
    FD="${WORK}/fishdata"
    mkdir -p "${FD}/fish/vendor_conf.d" "${FD}/fish/vendor_completions.d"
    cp "$FISH_CONF" "${FD}/fish/vendor_conf.d/"
    cp "${FISH_COMP}"/*.fish "${FD}/fish/vendor_completions.d/"

    # Every shipped file must parse. A syntax error in a vendor_conf.d file is a
    # broken shell for every fish user on the image.
    parse_bad=0
    for f in "$FISH_CONF" "${FISH_COMP}"/*.fish; do
        fish -n "$f" </dev/null >/dev/null 2>&1 || { parse_bad=1; echo "      $f"; }
    done
    [ "$parse_bad" -eq 0 ] && ok "every shipped fish file parses" \
                           || bad "every shipped fish file parses"

    fishrun() { # cwd extra-env… -- code
        local cwd="$1"; shift
        local -a extra=()
        while [ "$1" != "--" ]; do extra+=("$1"); shift; done
        shift
        (cd "$cwd" && env -i PATH="${BIN}:/usr/bin:/bin" HOME="${WORK}/home" \
            XDG_DATA_HOME="$FD" XDG_CONFIG_HOME="${WORK}/fishcfg" \
            "${extra[@]}" fish -c "$1" 2>&1)
    }

    # ── the shortcuts exist, and are the same six ────────────────────────────
    out="$(fishrun "$PROJ" -- 'for f in a aa al ad aw ap
    functions -q $f; and echo "have $f"; or echo "MISSING $f"
end')"
    if ! printf '%s' "$out" | grep -q MISSING; then
        ok "fish defines all six shortcuts"
    else
        bad "fish defines all six shortcuts"; printf '      %s\n' "$out"
    fi

    # ── they call the right thing ────────────────────────────────────────────
    : > "$CALLS"
    fishrun "$PROJ" -- 'a --agent claude "fix the tests"' >/dev/null
    grep -qx 'agent run --agent claude fix the tests' "$CALLS" \
        && ok "fish \`a\` runs \`apex agent run\` with its arguments" \
        || { bad "fish \`a\` runs \`apex agent run\` with its arguments"; sed 's/^/      /' "$CALLS"; }

    : > "$CALLS"
    fishrun "$PROJ" -- 'ap layout show' >/dev/null
    grep -qx 'project layout show' "$CALLS" \
        && ok "fish \`ap\` forwards to \`apex project\`" || bad "fish \`ap\` forwards to \`apex project\`"

    : > "$CALLS"
    out="$(fishrun "$PROJ" -- 'aw')"
    printf '%s' "$out" | grep -q 'usage: aw <worktree-name>' \
        && ok "fish \`aw\` with no worktree explains itself" \
        || { bad "fish \`aw\` with no worktree explains itself"; printf '      %s\n' "$out"; }

    : > "$CALLS"
    fishrun "$PROJ" -- 'aw feature-x "do the thing"' >/dev/null
    grep -qx 'agent run --worktree feature-x do the thing' "$CALLS" \
        && ok "fish \`aw\` puts the worktree on the command line" \
        || { bad "fish \`aw\` puts the worktree on the command line"; sed 's/^/      /' "$CALLS"; }

    # `aa` with no id: exactly one running session, so no id is needed. The stub
    # reports one, which is the case the shortcut exists for.
    : > "$CALLS"
    fishrun "$PROJ" -- 'aa' >/dev/null
    grep -qx 'agent attach 4' "$CALLS" \
        && ok "fish \`aa\` attaches to the only running session" \
        || { bad "fish \`aa\` attaches to the only running session"; sed 's/^/      /' "$CALLS"; }

    # ── the opt-out ──────────────────────────────────────────────────────────
    out="$(fishrun "$PROJ" APEX_NO_AGENT_ALIASES=1 -- 'functions -q a; and echo BAD; or echo gone
functions -q apex_agent_prompt; and echo prompt-kept; or echo BAD-PROMPT')"
    printf '%s' "$out" | grep -q '^gone$' && printf '%s' "$out" | grep -q 'prompt-kept' \
        && ok "APEX_NO_AGENT_ALIASES drops the shortcuts and keeps the prompt" \
        || { bad "APEX_NO_AGENT_ALIASES drops the shortcuts and keeps the prompt"; printf '      %s\n' "$out"; }

    out="$(fishrun "$PROJ" APEX_NO_AGENT_ALIASES=1 -- 'complete -C "apex agent attach "')"
    printf '%s' "$out" | grep -q '^4' \
        && ok "completion survives the opt-out" || bad "completion survives the opt-out"

    # ── no apex installed ────────────────────────────────────────────────────
    out="$( (cd "$PROJ" && env -i PATH="$NOAPEX" HOME="${WORK}/home" XDG_DATA_HOME="$FD" \
        XDG_CONFIG_HOME="${WORK}/fishcfg" "${NOAPEX}/fish" -c \
        'functions -q a; and echo BAD; or echo none; echo alive' 2>&1) )"
    printf '%s' "$out" | grep -q '^none$' && printf '%s' "$out" | grep -q '^alive$' \
        && ok "a machine with no apex gets no shortcuts and a working shell" \
        || { bad "a machine with no apex gets no shortcuts and a working shell"; printf '      %s\n' "$out"; }

    # ── the double-source guard ──────────────────────────────────────────────
    # Both fish itself and a user who copied the file into ~/.config/fish/conf.d
    # can source it. The second must be a no-op, not a redefinition.
    out="$(fishrun "$PROJ" -- "source '${FISH_CONF}'; echo sourced-twice-ok; functions -q a; and echo still-have-a")"
    printf '%s' "$out" | grep -q 'sourced-twice-ok' && printf '%s' "$out" | grep -q 'still-have-a' \
        && ok "sourcing the fish file twice is harmless" \
        || { bad "sourcing the fish file twice is harmless"; printf '      %s\n' "$out"; }

    # ── the prompt indicator ─────────────────────────────────────────────────
    got="$(fishrun "$PROJ" XDG_STATE_HOME="$STATE" -- 'apex_agent_prompt')"
    want="$(bash_prompt "$PROJ" "$STATE")"
    [ -n "$want" ] && [ "$got" = "$want" ] \
        && ok "the fish prompt is byte-identical to the bash one" \
        || { bad "the fish prompt is byte-identical to the bash one"
             printf '      fish: %s\n      bash: %s\n' "$got" "$want"; }

    # $OUTSIDE has exactly one session of its own, and $PROJ has three. Neither
    # may see the other's — which is the whole reason the prompt matches on the
    # recorded project root instead of just counting session files.
    got="$(fishrun "$OUTSIDE" XDG_STATE_HOME="$STATE" -- 'apex_agent_prompt')"
    want="$(bash_prompt "$OUTSIDE" "$STATE")"
    [ -n "$want" ] && [ "$got" = "$want" ] \
        && ok "another project's prompt counts only its own sessions" \
        || { bad "another project's prompt counts only its own sessions"
             printf '      fish: %s\n      bash: %s\n' "$got" "$want"; }

    got="$(fishrun "${WORK}" XDG_STATE_HOME="$STATE" -- 'apex_agent_prompt')"
    [ -z "$got" ] && ok "a directory no session is working in shows nothing" \
                  || bad "a directory no session is working in shows nothing (got '${got}')"

    got="$(fishrun "$PROJ" XDG_STATE_HOME="$EMPTY" -- 'apex_agent_prompt')"
    [ -z "$got" ] && ok "an empty session directory prints nothing" \
                  || bad "an empty session directory prints nothing (got '${got}')"

    got="$(fishrun "$PROJ" XDG_STATE_HOME="${WORK}/no-such-state" -- 'apex_agent_prompt; echo rc=$status')"
    [ "$got" = "rc=0" ] && ok "no state directory at all is not an error" \
                        || bad "no state directory at all is not an error (got '${got}')"

    # The property the prompt lives or dies by: it runs before every command, so
    # it must not fork. Asserted against the stub's own call log, not against
    # the source.
    : > "$CALLS"
    fishrun "$PROJ" XDG_STATE_HOME="$STATE" -- 'apex_agent_prompt' >/dev/null
    [ ! -s "$CALLS" ] && ok "the fish prompt never runs apex" \
                      || { bad "the fish prompt never runs apex"; sed 's/^/      /' "$CALLS"; }

    # ── completion ───────────────────────────────────────────────────────────
    comp() { fishrun "$PROJ" -- "complete -C \"$1\""; }

    printf '%s' "$(comp 'apex ')" | grep -q '^agent' \
        && ok "completion offers the top-level verbs" || bad "completion offers the top-level verbs"
    printf '%s' "$(comp 'apex agent ')" | grep -q '^attach' \
        && ok "completion offers the agent verbs" || bad "completion offers the agent verbs"
    out="$(comp 'apex agent attach ')"
    printf '%s' "$out" | grep -q '^4' && printf '%s' "$out" | grep -q '^7' \
        && ok "completion offers session ids, exited ones included" \
        || { bad "completion offers session ids, exited ones included"; printf '      %s\n' "$out"; }
    printf '%s' "$(comp 'apex agent default ')" | grep -q '^claude' \
        && ok "completion offers agent names with the default marker stripped" \
        || bad "completion offers agent names with the default marker stripped"
    printf '%s' "$(comp 'apex request ask ')" | grep -q 'pkg.install' \
        && ok "completion asks the CLI for the requestable verbs" \
        || bad "completion asks the CLI for the requestable verbs"
    printf '%s' "$(comp 'apex secret grant ')" | grep -q '^github' \
        && ok "completion asks the CLI for the stored services" \
        || bad "completion asks the CLI for the stored services"
    printf '%s' "$(comp 'apex secret grant github ')" | grep -q 'repo.read' \
        && ok "completion asks the CLI for the capability vocabulary" \
        || bad "completion asks the CLI for the capability vocabulary"
    printf '%s' "$(comp 'apex project layout ')" | grep -q '^restore' \
        && ok "completion offers the layout verbs" || bad "completion offers the layout verbs"
    printf '%s' "$(comp 'aa ')" | grep -q '^4' \
        && ok "the aa shortcut completes session ids" || bad "the aa shortcut completes session ids"
    printf '%s' "$(comp 'ad ')" | grep -q '^4' \
        && ok "the ad shortcut completes session ids" || bad "the ad shortcut completes session ids"
    printf '%s' "$(comp 'a --agent ')" | grep -q '^claude' \
        && ok "the a shortcut completes agent names" || bad "the a shortcut completes agent names"
    printf '%s' "$(comp 'ap layout ')" | grep -q '^restore' \
        && ok "the ap shortcut completes layout verbs" || bad "the ap shortcut completes layout verbs"

    # `list` is a verb under agent, project, request AND secret. A completion
    # that matched on "the word list was typed" would fire in all four.
    out="$(comp 'apex project ')"
    printf '%s' "$out" | grep -q '^worktrees' && ! printf '%s' "$out" | grep -q '^adapters' \
        && ok "the project verbs do not leak the agent verbs" \
        || { bad "the project verbs do not leak the agent verbs"; printf '      %s\n' "$out"; }

    # ── the runtime is down ──────────────────────────────────────────────────
    out="$( (cd "$PROJ" && env -i PATH="${DOWN}:/usr/bin:/bin" HOME="${WORK}/home" \
        XDG_DATA_HOME="$FD" XDG_CONFIG_HOME="${WORK}/fishcfg" fish -c \
        'complete -C "apex agent attach "' 2>&1) )"
    [ -z "$out" ] && ok "completion with the runtime down prints nothing at all" \
                  || { bad "completion with the runtime down prints nothing at all"; printf '      %s\n' "$out"; }
fi

# ─────────────────────────────────────────────────────────────────────────────
printf '\nshell-agent: %d passed, %d failed, %d skipped\n' "$pass" "$fail" "$skip"
if [ "$sections_run" -eq 0 ]; then
    echo "FATAL: every section skipped — this run asserted nothing" >&2
    exit 2
fi
[ "$fail" -eq 0 ]
