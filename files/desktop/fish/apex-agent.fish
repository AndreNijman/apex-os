# APEX-OS — agent shell integration for fish.
#
# The fish half of files/desktop/shell/agent.sh. It is a separate file and not
# a translation layer because fish is not a POSIX shell: `agent.sh` cannot be
# sourced here at all, and a wrapper that tried would be a second dialect to
# keep in step rather than one.
#
# What is shared with bash and zsh is the BEHAVIOUR, and the tests assert it
# rather than trusting this comment: the same six shortcuts, the same
# APEX_NO_AGENT_ALIASES opt-out, the same fork-free prompt indicator reading the
# same session records.
#
# Installed to /usr/share/fish/vendor_conf.d/, which is on fish's own
# $__fish_vendor_confdirs — the path fish sources for every shell, interactive
# or not, without anybody editing a dotfile. Completions live beside it in
# vendor_completions.d/ and are autoloaded by command name, so they cost
# nothing until somebody presses tab.
#
# Set APEX_NO_AGENT_ALIASES in ~/.config/fish/config.fish to skip the
# shortcuts while keeping completion.

# Nothing to do if the CLI is not installed (a partial image, a container).
# `if …; end` around the whole body rather than a bare `return`: this file is
# sourced, and getting the early-exit wrong breaks every fish shell on the
# machine. `command -q` is a builtin, so the guard costs no process.
if command -q apex

    # This file is sourced once per shell by fish itself, but a user who copies
    # it into ~/.config/fish/conf.d/ would get it twice. Guard, for the same
    # reason agent.sh does. Deliberately NOT exported: a child shell must source
    # the file afresh rather than inherit a guard that makes it skip.
    if not set -q _apex_agent_fish_sourced
        set -g _apex_agent_fish_sourced 1

        # ── completion helpers ──────────────────────────────────────────────
        # Defined before the alias guard, and outside it: completion must keep
        # working for somebody who set APEX_NO_AGENT_ALIASES, and `aa` needs
        # _apex_only_session even though the completion files in
        # vendor_completions.d/ are autoloaded much later.
        #
        # Every one of these is allowed to fail silently. Completion must never
        # print an error or hang because the runtime is not running.

        # The id of the single running session, or nothing when there is not
        # exactly one. Used by `aa` so the common case needs no id, without ever
        # attaching to an arbitrary session when the answer is ambiguous.
        function _apex_only_session --description 'the id of the one running session, if there is exactly one'
            set -l ids (_apex_session_ids_running)
            test (count $ids) -eq 1; or return 1
            echo $ids[1]
        end

        function _apex_session_ids_running --description 'ids of sessions that have not exited'
            apex agent list --json 2>/dev/null \
                | string match -rag '"id"\s*:\s*([0-9]+)'
        end

        function _apex_session_ids --description 'ids of every session, exited included'
            apex agent list --all --json 2>/dev/null \
                | string match -rag '"id"\s*:\s*([0-9]+)'
        end

        function _apex_agent_names --description 'installed agent adapters'
            apex agent adapters 2>/dev/null | tail -n +2 \
                | string replace -r '\s.*$' '' \
                | string replace -a '*' '' \
                | string match -rv '^$'
        end

        # Stored secret services, and the capabilities that can be granted. Both
        # asked of the CLI: the capability set is a security boundary, and a
        # stale copy of it in a completion list misrepresents what the broker
        # accepts.
        function _apex_secret_services --description 'services the broker holds a secret for'
            apex secret list --json 2>/dev/null \
                | string match -rag '"service"\s*:\s*"([^"]*)"'
        end

        function _apex_secret_capabilities --description 'capabilities the broker can grant'
            apex secret capabilities 2>/dev/null | string match -rg '^  ([a-z][^ ]*)'
        end

        function _apex_request_ids --description 'privilege-request ids'
            apex request list --all --json 2>/dev/null \
                | string match -rag '"id"\s*:\s*([0-9]+)'
        end

        function _apex_layout_templates --description 'terminal layout templates'
            apex project layout templates 2>/dev/null | tail -n +2 \
                | string replace -r '\s.*$' '' | string match -rv '^$'
        end

        # The requestable verbs, asked of the CLI rather than duplicated here.
        # The vocabulary is a security boundary, so a completion list that
        # drifts out of step with it would offer operations the daemon refuses
        # — or, worse, stop offering one it accepts and make it look
        # unsupported.
        function _apex_request_verbs --description 'operations that can be requested'
            apex request verbs 2>/dev/null | string match -rg '^  ([a-z][^ ]*)'
        end

        if not set -q APEX_NO_AGENT_ALIASES
            # Start an agent here. `a` with no arguments opens the agent
            # interactively; `a "fix the tests"` gives it an opening
            # instruction.
            function a --description 'start an agent in this directory'
                apex agent run $argv
            end

            # Reattach. `aa` with no id attaches to the only running session,
            # which is the common case; with several it lists them rather than
            # guessing.
            function aa --description 'attach to an agent session'
                if test (count $argv) -gt 0
                    apex agent attach $argv
                    return
                end
                set -l only (_apex_only_session)
                if test -z "$only"
                    apex agent list
                    return 1
                end
                apex agent attach $only
            end

            function al --description 'list agent sessions'
                apex agent list $argv
            end

            function ad --description 'what an agent changed'
                apex agent diff $argv
            end

            function aw --description 'start an agent in a worktree'
                if test (count $argv) -eq 0
                    echo "usage: aw <worktree-name> [prompt]" >&2
                    return 2
                end
                set -l wt $argv[1]
                apex agent run --worktree $wt $argv[2..]
            end

            function ap --description 'project commands'
                apex project $argv
            end
        end

        # ── prompt indicator ────────────────────────────────────────────────
        # Opt-in, and FORK-FREE, which is the only way a prompt hook is
        # acceptable: it runs before every single command. Everything below is a
        # fish builtin — `read` with a file redirect, `string match` with named
        # capture groups, `test`. No `apex`, no socket round trip, no `git`, no
        # command substitution.
        #
        # Relevance is decided from $PWD against each session's recorded project
        # root, so there is no need to ask git where we are.
        #
        # Usage — add to ~/.config/fish/config.fish:
        #     function fish_prompt
        #         apex_agent_prompt
        #         # …your prompt…
        #     end
        function apex_agent_prompt --description 'agent activity for the project containing $PWD'
            set -l dir
            if set -q XDG_STATE_HOME; and test -n "$XDG_STATE_HOME"
                set dir $XDG_STATE_HOME/apex/agent/sessions
            else
                set dir $HOME/.local/state/apex/agent/sessions
            end
            test -d $dir; or return 0

            set -l working 0
            set -l waiting 0
            set -l attention 0

            # An unmatched glob yields nothing here rather than an error: fish
            # only complains about a wildcard that matched nothing when it is an
            # argument to a command, and this is a `for` header.
            for f in $dir/*.json
                # `read -z` reads the whole file with the redirect done by fish
                # itself. No `cat`, no subshell.
                set -l text
                read -z text <$f
                or continue

                # Only sessions whose project contains $PWD. A session with no
                # project is skipped rather than shown everywhere.
                string match -rq '"project": "(?<_apex_p>[^"]*)"' -- $text
                or continue
                test -n "$_apex_p"; or continue
                if test "$PWD" != "$_apex_p"; and not string match -q -- "$_apex_p/*" $PWD
                    continue
                end

                # A finished session is not worth a prompt indicator.
                string match -rq '"exit_code": (?<_apex_x>[^,\n]*)' -- $text
                or continue
                test "$_apex_x" = null; or continue

                string match -rq '"state": "(?<_apex_s>[^"]*)"' -- $text
                or continue
                switch $_apex_s
                    case working
                        set working (math $working + 1)
                    case waiting_for_user
                        set waiting (math $waiting + 1)
                    case permission_request
                        set attention (math $attention + 1)
                end
            end

            set -l out ""
            test $working -gt 0; and set out "$out󰜎$working"
            test $waiting -gt 0; and set out "$out󰅺$waiting"
            test $attention -gt 0; and set out "$out󰌾$attention"
            test -n "$out"; and printf '%s ' $out
            return 0
        end
    end
end
