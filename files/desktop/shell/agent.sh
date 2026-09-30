# Rime OS — agent shell integration. Sourced by both bash and zsh.
#
# shellcheck shell=bash
# There is no shebang because nothing executes this file — both shells source
# it. shellcheck has to be told which dialect to read it as, and bash is the
# only one of the two it can read at all. The zsh half is guarded by
# $ZSH_VERSION and carries its own directive where that guard begins.
#
# Provides the universal `a` command and its siblings, plus completion for the
# things that are worth completing: session ids and agent names.
#
# `a` maps to whichever upstream agent the user selected with
# `rime agent default`. It is deliberately a thin shell function, not a binary:
# the roadmap's rule is that the short command must be transparent, and a
# function is something the user can read with `type a` and override in
# ~/.zshrc.local without fighting the OS.
#
# Nothing here is required. Every shortcut has a full `rime agent …` form, and
# running `claude`, `opencode`, `codex` or `gemini` directly keeps working
# exactly as it did — that is the non-negotiable escape hatch, not a fallback.
#
# Set RIME_NO_AGENT_ALIASES=1 in ~/.zshrc.local or ~/.bashrc to skip the
# shortcuts while keeping completion.

# Nothing to do if the CLI is not installed (a partial image, a container).
command -v rime >/dev/null 2>&1 || return 0

# This file is sourced from more than one place that can overlap: /etc/bashrc and
# /etc/zshrc source it for every interactive shell, and a seeded ~/.zshrc sources
# it again (that file is read after /etc/zshrc). Guard so the second source is a
# no-op instead of redefining every function and re-registering completion. A
# plain shell variable, deliberately NOT exported: a child shell must source the
# file afresh and must not inherit a guard that makes it skip.
if [ -n "${_RIME_AGENT_SH_SOURCED}" ]; then
    return 0
fi
_RIME_AGENT_SH_SOURCED=1

# APEX_NO_AGENT_ALIASES is the same opt-out under the name a user set before
# the rename, in a ~/.zshrc.local or ~/.bashrc this image never rewrites.
if [ -z "${RIME_NO_AGENT_ALIASES}${APEX_NO_AGENT_ALIASES:-}" ]; then  # rime-rename: keep — opt-out users already set
    # Start an agent here. `a` with no arguments opens the agent interactively;
    # `a "fix the tests"` gives it an opening instruction.
    a() { rime agent run "$@"; }

    # Reattach. `aa` with no id attaches to the only running session, which is
    # the common case; with several it lists them rather than guessing.
    aa() {
        if [ "$#" -gt 0 ]; then
            rime agent attach "$@"
            return
        fi
        _rime_only_session >/dev/null || { rime agent list; return 1; }
        rime agent attach "$(_rime_only_session)"
    }

    al() { rime agent list "$@"; }
    ad() { rime agent diff "$@"; }
    # The worktree's name is the session's name too, unless the command line or
    # $RIME_AGENT_NAME already gives one — so `aw issue-217` shows up as
    # "issue-217" in the Agent Center and on the phone without anybody typing
    # it twice. A name past the 64-character limit is left off rather than
    # passed along to be refused: the worktree is still what was asked for.
    aw() {
        if [ "$#" -eq 0 ]; then
            echo "usage: aw <worktree-name> [prompt]" >&2
            return 2
        fi
        _rime_wt="$1"
        shift
        if _rime_names_itself "$@" || [ "${#_rime_wt}" -gt 64 ]; then
            rime agent run --worktree "$_rime_wt" "$@"
        else
            rime agent run --worktree "$_rime_wt" --name "$_rime_wt" "$@"
        fi
        _rime_rc=$?
        unset _rime_wt
        return "$_rime_rc"
    }
    ap() { rime project "$@"; }
fi

# Whether a `rime agent run` command line already names its session, so `aw`
# does not name it a second time (`--name` given twice is refused). Stops at
# `--`: everything after it belongs to the agent binary, not to rime.
_rime_names_itself() {
    [ -n "${RIME_AGENT_NAME:-}" ] && return 0
    for _rime_arg in "$@"; do
        case "$_rime_arg" in
            --) break ;;
            --name|--name=*|-n|-n?*) unset _rime_arg; return 0 ;;
        esac
    done
    unset _rime_arg
    return 1
}

# The id of the single running session, or failure when there is not exactly
# one. Used by `aa` so the common case needs no id, without ever attaching to an
# arbitrary session when the answer is ambiguous.
_rime_only_session() {
    _rime_ids="$(rime agent list --json 2>/dev/null \
        | sed -n 's/.*"id"[[:space:]]*:[[:space:]]*\([0-9]\{1,\}\).*/\1/p')"
    [ -n "$_rime_ids" ] || { unset _rime_ids; return 1; }
    if [ "$(printf '%s\n' "$_rime_ids" | wc -l)" -ne 1 ]; then
        unset _rime_ids
        return 1
    fi
    printf '%s\n' "$_rime_ids"
    unset _rime_ids
    return 0
}

# Session ids for completion. Silent and fast-failing: completion must never
# print an error or hang when the runtime is not running.
_rime_session_ids() {
    rime agent list --all --json 2>/dev/null \
        | sed -n 's/.*"id"[[:space:]]*:[[:space:]]*\([0-9]\{1,\}\).*/\1/p'
}

_rime_agent_names() {
    rime agent adapters 2>/dev/null | awk 'NR>1 {print $1}' | tr -d '*'
}

# ── prompt indicator ────────────────────────────────────────────────────────
# §3: "Optional prompt indicator showing active agent state for the current
# project."
#
# Opt-in, and FORK-FREE, which is the only way a prompt hook is acceptable: it
# runs before every single command. So this reads the session records the daemon
# already writes on each state change, with `$(<file)` and parameter expansion —
# no `rime`, no socket round trip, no `git`, no `sed`. A prompt that costs three
# forks per command is a prompt people turn off, and then the feature does not
# exist.
#
# Relevance is decided from $PWD against each session's recorded project root,
# so there is no need to ask git where we are.
#
# Usage — add to ~/.zshrc.local or ~/.bashrc:
#     PS1='$(rime_agent_prompt)'"$PS1"      # bash
#     setopt PROMPT_SUBST                    # zsh
#     PROMPT='$(rime_agent_prompt)'"$PROMPT"
rime_agent_prompt() {
    local dir="${XDG_STATE_HOME:-$HOME/.local/state}/rime/agent/sessions"
    [ -d "$dir" ] || return 0

    local working=0 waiting=0 attention=0 f text root state exited
    for f in "$dir"/*.json; do
        [ -f "$f" ] || continue
        text="$(<"$f")"

        # Only sessions whose project contains $PWD. A session with no project
        # is skipped rather than shown everywhere.
        root="${text#*\"project\": \"}"
        [ "$root" = "$text" ] && continue
        root="${root%%\"*}"
        [ -n "$root" ] || continue
        case "$PWD" in
            "$root"|"$root"/*) ;;
            *) continue ;;
        esac

        # A finished session is not worth a prompt indicator.
        exited="${text#*\"exit_code\": }"
        exited="${exited%%,*}"
        [ "$exited" = "null" ] || continue

        state="${text#*\"state\": \"}"
        state="${state%%\"*}"
        case "$state" in
            working)                      working=$((working + 1)) ;;
            waiting_for_user)             waiting=$((waiting + 1)) ;;
            permission_request)           attention=$((attention + 1)) ;;
        esac
    done

    local out=""
    [ "$working"   -gt 0 ] && out="${out}󰜎${working}"
    [ "$waiting"   -gt 0 ] && out="${out}󰅺${waiting}"
    [ "$attention" -gt 0 ] && out="${out}󰌾${attention}"
    [ -n "$out" ] && printf '%s ' "$out"
    return 0
}

# Stored secret services, and the capabilities that can be granted. Both asked
# of the CLI: the capability set is a security boundary, and a stale copy of it
# in a completion list misrepresents what the broker accepts.
_rime_secret_services() {
    rime secret list --json 2>/dev/null \
        | sed -n 's/.*"service"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p'
}

_rime_secret_capabilities() {
    rime secret capabilities 2>/dev/null | awk '/^  [a-z]/ {print $1}'
}

# Privilege-request ids, for `rime request approve|deny|show`.
_rime_request_ids() {
    rime request list --all --json 2>/dev/null \
        | sed -n 's/.*"id"[[:space:]]*:[[:space:]]*\([0-9]\{1,\}\).*/\1/p'
}

# Terminal layout templates, asked of the CLI. A hardcoded list here would go
# stale the moment a template is added, and offering one that does not exist is
# how a completion teaches somebody a command that fails.
_rime_layout_templates() {
    rime project layout templates 2>/dev/null | awk 'NR>1 {print $1}'
}

# The requestable verbs, asked of the CLI rather than duplicated here. The
# vocabulary is a security boundary, so a completion list that drifts out of
# step with it would offer operations the daemon refuses — or, worse, stop
# offering one it accepts and make it look unsupported.
_rime_request_verbs() {
    rime request verbs 2>/dev/null | awk '/^  [a-z]/ {print $1}'
}

# ── bash completion ─────────────────────────────────────────────────────────
# shellcheck disable=SC2207  # `COMPREPLY=($(compgen …))` is THE bash-completion
# idiom, and the split on IFS is the point: COMPREPLY is an array of candidate
# words. Quoting it, which is what SC2207 asks for, would make every completion
# offer one candidate that is all the candidates joined by spaces. `mapfile`,
# the other suggestion, cannot be used in a function that must also work under
# `set -u` in every bash the image ships. 21 occurrences, all the same shape.
if [ -n "${BASH_VERSION}" ]; then
    _rime_agent_complete() {
        local cur prev verb
        cur="${COMP_WORDS[COMP_CWORD]}"
        prev="${COMP_WORDS[COMP_CWORD-1]}"
        verb="${COMP_WORDS[2]}"

        case "$prev" in
            --agent|-a) COMPREPLY=($(compgen -W "$(_rime_agent_names)" -- "$cur")); return ;;
            --to|-t) COMPREPLY=($(compgen -W "$(_rime_agent_names)" -- "$cur")); return ;;
            --sandbox|-s) COMPREPLY=($(compgen -W "strict project unrestricted" -- "$cur")); return ;;
            # A name is free text: offering anything here would be a guess.
            --name|-n) COMPREPLY=(); return ;;
        esac

        if [ "$COMP_CWORD" -eq 2 ]; then
            COMPREPLY=($(compgen -W "run list attach input rename handoff pause resume kill logs \
                status default adapters diff undo checkpoint event rm prune enable" -- "$cur"))
            return
        fi

        case "$verb" in
            attach|input|rename|handoff|pause|resume|kill|logs|rm|status|diff|undo)
                COMPREPLY=($(compgen -W "$(_rime_session_ids)" -- "$cur")) ;;
            default)
                COMPREPLY=($(compgen -W "$(_rime_agent_names)" -- "$cur")) ;;
            event)
                COMPREPLY=($(compgen -W "working waiting_for_user permission_request \
                    complete failed" -- "$cur")) ;;
        esac
    }

    _rime_request_complete() {
        local cur="${COMP_WORDS[COMP_CWORD]}" verb="${COMP_WORDS[2]}"
        if [ "$COMP_CWORD" -eq 2 ]; then
            COMPREPLY=($(compgen -W "ask list pending show approve deny verbs \
                grants revoke audit" -- "$cur"))
            return
        fi
        case "$verb" in
            ask)      COMPREPLY=($(compgen -W "$(_rime_request_verbs)" -- "$cur")) ;;
            show|approve|deny)
                      COMPREPLY=($(compgen -W "$(_rime_request_ids)" -- "$cur")) ;;
        esac
    }

    _rime_project_complete() {
        local cur="${COMP_WORDS[COMP_CWORD]}" verb="${COMP_WORDS[2]}"
        if [ "$COMP_CWORD" -eq 2 ]; then
            COMPREPLY=($(compgen -W "list info worktrees checkpoints remove \
                forget env layout switch" -- "$cur"))
            return
        fi
        case "$verb" in
            layout)
                if [ "$COMP_CWORD" -eq 3 ]; then
                    COMPREPLY=($(compgen -W "save show restore forget \
                        templates open" -- "$cur"))
                elif [ "${COMP_WORDS[3]}" = "open" ]; then
                    COMPREPLY=($(compgen -W "$(_rime_layout_templates)" -- "$cur"))
                fi ;;
        esac
    }

    _rime_secret_complete() {
        local cur="${COMP_WORDS[COMP_CWORD]}" verb="${COMP_WORDS[2]}"
        if [ "$COMP_CWORD" -eq 2 ]; then
            COMPREPLY=($(compgen -W "add list remove capabilities grant revoke \
                grants use audit" -- "$cur"))
            return
        fi
        case "$verb" in
            # Service names, from the CLI rather than a hardcoded list.
            remove|grant|revoke|use)
                if [ "$COMP_CWORD" -eq 3 ]; then
                    COMPREPLY=($(compgen -W "$(_rime_secret_services)" -- "$cur"))
                elif [ "$COMP_CWORD" -eq 4 ]; then
                    COMPREPLY=($(compgen -W "$(_rime_secret_capabilities)" -- "$cur"))
                fi ;;
        esac
    }

    _rime_complete() {
        if [ "${COMP_WORDS[1]}" = "secret" ]; then
            _rime_secret_complete
            return
        fi
        if [ "${COMP_WORDS[1]}" = "project" ]; then
            _rime_project_complete
            return
        fi
        if [ "${COMP_WORDS[1]}" = "agent" ]; then
            _rime_agent_complete
            return
        fi
        if [ "${COMP_WORDS[1]}" = "request" ]; then
            _rime_request_complete
            return
        fi
        if [ "$COMP_CWORD" -eq 1 ]; then
            COMPREPLY=($(compgen -W "status tier profile battery fan game agent project \
                request fingerprint pin rollback update shell metrics doctor image install \
                remove search repo pkg" -- "${COMP_WORDS[1]}"))
        fi
    }
    complete -F _rime_complete rime

    _a_complete() {
        local cur="${COMP_WORDS[COMP_CWORD]}"
        local prev="${COMP_WORDS[COMP_CWORD-1]}"
        case "$prev" in
            --agent|-a) COMPREPLY=($(compgen -W "$(_rime_agent_names)" -- "$cur")) ;;
            --sandbox|-s) COMPREPLY=($(compgen -W "strict project unrestricted" -- "$cur")) ;;
            --name|-n) COMPREPLY=() ;;
            *) COMPREPLY=($(compgen -W "--agent --name --sandbox --worktree --checkpoint --detach" -- "$cur")) ;;
        esac
    }
    complete -F _a_complete a

    # `aa`, `ad` and friends take a session id as their first argument. They
    # cannot reuse _rime_agent_complete: that reads the verb from
    # COMP_WORDS[2], which for `aa 4` is not a verb at all.
    _rime_session_complete() {
        COMPREPLY=($(compgen -W "$(_rime_session_ids)" -- "${COMP_WORDS[COMP_CWORD]}"))
    }
    complete -F _rime_session_complete aa
    complete -F _rime_session_complete ad
fi

# ── zsh completion ──────────────────────────────────────────────────────────
# Plain `compctl`-free completion using compdef, which the seeded zshrc has
# already initialised by the time this file is sourced.
# shellcheck disable=SC2296,SC2206,SC2154,SC2034  # zsh, which shellcheck cannot
# parse. `${(f)…}` is zsh's split-on-newline flag and reads to a bash parser as
# a parameter expansion starting with `(`; `$words` is zsh's completion-state
# array, set by the completion system rather than by this file; and the `local
# -a` arrays here are consumed by `_describe`, which a bash parser does not know
# passes them by NAME. Everything in this block is guarded by $ZSH_VERSION, so
# bash never reaches it. The alternative — a second file — would split one
# completion model across two, which is the thing this file exists to avoid.
if [ -n "${ZSH_VERSION}" ]; then
    _rime_agent_zsh() {
        local -a verbs
        verbs=(run list attach input rename handoff pause resume kill logs status default
               adapters diff undo checkpoint event rm prune enable)
        if (( CURRENT == 3 )); then
            _describe 'agent verb' verbs
            return
        fi
        case "${words[3]}" in
            attach|input|rename|handoff|pause|resume|kill|logs|rm|status|diff|undo)
                local -a ids
                ids=(${(f)"$(_rime_session_ids)"})
                _describe 'session' ids ;;
            default)
                local -a names
                names=(${(f)"$(_rime_agent_names)"})
                _describe 'agent' names ;;
            event)
                local -a states
                states=(working waiting_for_user permission_request complete failed)
                _describe 'state' states ;;
        esac
    }
    # Only register when the completion system is actually loaded; sourcing this
    # from a non-interactive shell must not error.
    _rime_request_zsh() {
        local -a verbs
        verbs=(ask list pending show approve deny verbs grants revoke audit)
        if (( CURRENT == 3 )); then
            _describe 'request verb' verbs
            return
        fi
        case "${words[3]}" in
            ask)
                local -a ops
                ops=(${(f)"$(_rime_request_verbs)"})
                _describe 'operation' ops ;;
            show|approve|deny)
                local -a ids
                ids=(${(f)"$(_rime_request_ids)"})
                _describe 'request' ids ;;
        esac
    }
    _rime_secret_zsh() {
        local -a verbs
        verbs=(add list remove capabilities grant revoke grants use audit)
        if (( CURRENT == 3 )); then
            _describe 'secret verb' verbs
            return
        fi
        case "${words[3]}" in
            remove|grant|revoke|use)
                if (( CURRENT == 4 )); then
                    local -a svcs
                    svcs=(${(f)"$(_rime_secret_services)"})
                    _describe 'service' svcs
                elif (( CURRENT == 5 )); then
                    local -a caps
                    caps=(${(f)"$(_rime_secret_capabilities)"})
                    _describe 'capability' caps
                fi ;;
        esac
    }

    _rime_project_zsh() {
        local -a verbs
        verbs=(list info worktrees checkpoints remove forget env layout switch)
        if (( CURRENT == 3 )); then
            _describe 'project verb' verbs
            return
        fi
        if [[ "${words[3]}" == layout ]]; then
            if (( CURRENT == 4 )); then
                local -a acts
                acts=(save show restore forget templates open)
                _describe 'layout verb' acts
            elif [[ "${words[4]}" == open ]]; then
                local -a tpl
                tpl=(${(f)"$(_rime_layout_templates)"})
                _describe 'template' tpl
            fi
        fi
    }
    if whence compdef >/dev/null 2>&1; then
        compdef _rime_agent_zsh 'rime agent'
        compdef _rime_request_zsh 'rime request'
        compdef _rime_project_zsh 'rime project'
        compdef _rime_secret_zsh 'rime secret'
    fi
fi
