# APEX-OS — agent shell integration. Sourced by both bash and zsh.
#
# Provides the universal `a` command and its siblings, plus completion for the
# things that are worth completing: session ids and agent names.
#
# `a` maps to whichever upstream agent the user selected with
# `apex agent default`. It is deliberately a thin shell function, not a binary:
# the roadmap's rule is that the short command must be transparent, and a
# function is something the user can read with `type a` and override in
# ~/.zshrc.local without fighting the OS.
#
# Nothing here is required. Every shortcut has a full `apex agent …` form, and
# running `claude`, `opencode`, `codex` or `gemini` directly keeps working
# exactly as it did — that is the non-negotiable escape hatch, not a fallback.
#
# Set APEX_NO_AGENT_ALIASES=1 in ~/.zshrc.local or ~/.bashrc to skip the
# shortcuts while keeping completion.

# Nothing to do if the CLI is not installed (a partial image, a container).
command -v apex >/dev/null 2>&1 || return 0

if [ -z "${APEX_NO_AGENT_ALIASES}" ]; then
    # Start an agent here. `a` with no arguments opens the agent interactively;
    # `a "fix the tests"` gives it an opening instruction.
    a() { apex agent run "$@"; }

    # Reattach. `aa` with no id attaches to the only running session, which is
    # the common case; with several it lists them rather than guessing.
    aa() {
        if [ "$#" -gt 0 ]; then
            apex agent attach "$@"
            return
        fi
        _apex_only_session >/dev/null || { apex agent list; return 1; }
        apex agent attach "$(_apex_only_session)"
    }

    al() { apex agent list "$@"; }
    ad() { apex agent diff "$@"; }
    aw() {
        if [ "$#" -eq 0 ]; then
            echo "usage: aw <worktree-name> [prompt]" >&2
            return 2
        fi
        _apex_wt="$1"
        shift
        apex agent run --worktree "$_apex_wt" "$@"
        unset _apex_wt
    }
    ap() { apex project "$@"; }
fi

# The id of the single running session, or failure when there is not exactly
# one. Used by `aa` so the common case needs no id, without ever attaching to an
# arbitrary session when the answer is ambiguous.
_apex_only_session() {
    _apex_ids="$(apex agent list --json 2>/dev/null \
        | sed -n 's/.*"id"[[:space:]]*:[[:space:]]*\([0-9]\{1,\}\).*/\1/p')"
    [ -n "$_apex_ids" ] || { unset _apex_ids; return 1; }
    if [ "$(printf '%s\n' "$_apex_ids" | wc -l)" -ne 1 ]; then
        unset _apex_ids
        return 1
    fi
    printf '%s\n' "$_apex_ids"
    unset _apex_ids
    return 0
}

# Session ids for completion. Silent and fast-failing: completion must never
# print an error or hang when the runtime is not running.
_apex_session_ids() {
    apex agent list --all --json 2>/dev/null \
        | sed -n 's/.*"id"[[:space:]]*:[[:space:]]*\([0-9]\{1,\}\).*/\1/p'
}

_apex_agent_names() {
    apex agent adapters 2>/dev/null | awk 'NR>1 {print $1}' | tr -d '*'
}

# Privilege-request ids, for `apex request approve|deny|show`.
_apex_request_ids() {
    apex request list --all --json 2>/dev/null \
        | sed -n 's/.*"id"[[:space:]]*:[[:space:]]*\([0-9]\{1,\}\).*/\1/p'
}

# The requestable verbs, asked of the CLI rather than duplicated here. The
# vocabulary is a security boundary, so a completion list that drifts out of
# step with it would offer operations the daemon refuses — or, worse, stop
# offering one it accepts and make it look unsupported.
_apex_request_verbs() {
    apex request verbs 2>/dev/null | awk '/^  [a-z]/ {print $1}'
}

# ── bash completion ─────────────────────────────────────────────────────────
if [ -n "${BASH_VERSION}" ]; then
    _apex_agent_complete() {
        local cur prev verb
        cur="${COMP_WORDS[COMP_CWORD]}"
        prev="${COMP_WORDS[COMP_CWORD-1]}"
        verb="${COMP_WORDS[2]}"

        case "$prev" in
            --agent|-a) COMPREPLY=($(compgen -W "$(_apex_agent_names)" -- "$cur")); return ;;
            --sandbox|-s) COMPREPLY=($(compgen -W "strict project unrestricted" -- "$cur")); return ;;
        esac

        if [ "$COMP_CWORD" -eq 2 ]; then
            COMPREPLY=($(compgen -W "run list attach pause resume kill logs status \
                default adapters diff undo checkpoint event rm prune" -- "$cur"))
            return
        fi

        case "$verb" in
            attach|pause|resume|kill|logs|rm|status|diff|undo)
                COMPREPLY=($(compgen -W "$(_apex_session_ids)" -- "$cur")) ;;
            default)
                COMPREPLY=($(compgen -W "$(_apex_agent_names)" -- "$cur")) ;;
            event)
                COMPREPLY=($(compgen -W "working waiting_for_user permission_request \
                    complete failed" -- "$cur")) ;;
        esac
    }

    _apex_request_complete() {
        local cur="${COMP_WORDS[COMP_CWORD]}" verb="${COMP_WORDS[2]}"
        if [ "$COMP_CWORD" -eq 2 ]; then
            COMPREPLY=($(compgen -W "ask list pending show approve deny verbs \
                grants revoke audit" -- "$cur"))
            return
        fi
        case "$verb" in
            ask)      COMPREPLY=($(compgen -W "$(_apex_request_verbs)" -- "$cur")) ;;
            show|approve|deny)
                      COMPREPLY=($(compgen -W "$(_apex_request_ids)" -- "$cur")) ;;
        esac
    }

    _apex_project_complete() {
        local cur="${COMP_WORDS[COMP_CWORD]}" verb="${COMP_WORDS[2]}"
        if [ "$COMP_CWORD" -eq 2 ]; then
            COMPREPLY=($(compgen -W "list info worktrees checkpoints remove \
                forget layout" -- "$cur"))
            return
        fi
        case "$verb" in
            layout) COMPREPLY=($(compgen -W "save show restore forget" -- "$cur")) ;;
        esac
    }

    _apex_complete() {
        if [ "${COMP_WORDS[1]}" = "project" ]; then
            _apex_project_complete
            return
        fi
        if [ "${COMP_WORDS[1]}" = "agent" ]; then
            _apex_agent_complete
            return
        fi
        if [ "${COMP_WORDS[1]}" = "request" ]; then
            _apex_request_complete
            return
        fi
        if [ "$COMP_CWORD" -eq 1 ]; then
            COMPREPLY=($(compgen -W "status tier profile battery fan game agent project \
                request fingerprint pin rollback update shell metrics doctor image install \
                remove search repo pkg" -- "${COMP_WORDS[1]}"))
        fi
    }
    complete -F _apex_complete apex

    _a_complete() {
        local cur="${COMP_WORDS[COMP_CWORD]}"
        local prev="${COMP_WORDS[COMP_CWORD-1]}"
        case "$prev" in
            --agent|-a) COMPREPLY=($(compgen -W "$(_apex_agent_names)" -- "$cur")) ;;
            --sandbox|-s) COMPREPLY=($(compgen -W "strict project unrestricted" -- "$cur")) ;;
            *) COMPREPLY=($(compgen -W "--agent --sandbox --worktree --checkpoint --detach" -- "$cur")) ;;
        esac
    }
    complete -F _a_complete a

    # `aa`, `ad` and friends take a session id as their first argument. They
    # cannot reuse _apex_agent_complete: that reads the verb from
    # COMP_WORDS[2], which for `aa 4` is not a verb at all.
    _apex_session_complete() {
        COMPREPLY=($(compgen -W "$(_apex_session_ids)" -- "${COMP_WORDS[COMP_CWORD]}"))
    }
    complete -F _apex_session_complete aa
    complete -F _apex_session_complete ad
fi

# ── zsh completion ──────────────────────────────────────────────────────────
# Plain `compctl`-free completion using compdef, which the seeded zshrc has
# already initialised by the time this file is sourced.
if [ -n "${ZSH_VERSION}" ]; then
    _apex_agent_zsh() {
        local -a verbs
        verbs=(run list attach pause resume kill logs status default adapters
               diff undo checkpoint event rm prune)
        if (( CURRENT == 3 )); then
            _describe 'agent verb' verbs
            return
        fi
        case "${words[3]}" in
            attach|pause|resume|kill|logs|rm|status|diff|undo)
                local -a ids
                ids=(${(f)"$(_apex_session_ids)"})
                _describe 'session' ids ;;
            default)
                local -a names
                names=(${(f)"$(_apex_agent_names)"})
                _describe 'agent' names ;;
            event)
                local -a states
                states=(working waiting_for_user permission_request complete failed)
                _describe 'state' states ;;
        esac
    }
    # Only register when the completion system is actually loaded; sourcing this
    # from a non-interactive shell must not error.
    _apex_request_zsh() {
        local -a verbs
        verbs=(ask list pending show approve deny verbs grants revoke audit)
        if (( CURRENT == 3 )); then
            _describe 'request verb' verbs
            return
        fi
        case "${words[3]}" in
            ask)
                local -a ops
                ops=(${(f)"$(_apex_request_verbs)"})
                _describe 'operation' ops ;;
            show|approve|deny)
                local -a ids
                ids=(${(f)"$(_apex_request_ids)"})
                _describe 'request' ids ;;
        esac
    }
    _apex_project_zsh() {
        local -a verbs
        verbs=(list info worktrees checkpoints remove forget layout)
        if (( CURRENT == 3 )); then
            _describe 'project verb' verbs
            return
        fi
        if [[ "${words[3]}" == layout ]]; then
            local -a acts
            acts=(save show restore forget)
            _describe 'layout verb' acts
        fi
    }
    if whence compdef >/dev/null 2>&1; then
        compdef _apex_agent_zsh 'apex agent'
        compdef _apex_request_zsh 'apex request'
        compdef _apex_project_zsh 'apex project'
    fi
fi
