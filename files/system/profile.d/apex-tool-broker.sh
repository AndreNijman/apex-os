# APEX: put the brokered `wrangler` and `terraform` ahead of the real ones for
#
# shellcheck shell=sh
# No shebang because nothing executes this: /etc/profile.d is SOURCED, and by
# whichever shell the login is running. So the dialect has to be declared, and
# `sh` is the right one to declare — it is the strictest of the shells that can
# source this, and linting it as bash would let a bashism through that a dash
# login would then choke on.
# a PERSON working inside a managed session.
#
# THIS IS NOT WHAT ROUTES AN AGENT'S COMMANDS, and it must not be mistaken for
# it. /etc/profile.d/*.sh is read by a *login* shell. An agent's tool calls are
# `bash -c '…'` — non-login, non-interactive — and never read this file. What
# routes those is apex-agentd: it symlinks the same shims into the session's
# own scratch `bin`, which is the directory it puts first on the session's
# PATH, next to the `git` shim that has always been there. See
# `install_session_bin` in apexd/apex-agentd/src/session.rs.
#
# This file is for the other case: someone opens a terminal inside a managed
# session and types `wrangler deploy` themselves. Without it that command would
# reach the real wrangler with no credential, which works but is confusing.
#
# Only inside an agent session, deliberately. $APEX_AGENT_SESSION is set by
# apex-agentd in every session it starts and by nothing else, so a person's own
# shell is untouched: someone who has exported their own CLOUDFLARE_API_TOKEN
# and wants to run wrangler directly goes on doing exactly that. Putting this
# on everyone's PATH would take a working tool away from the owner of the
# machine to protect them from a credential they already have.
if [ -n "${APEX_AGENT_SESSION:-}" ] && [ -d /usr/libexec/apex/tools ]; then
    case ":${PATH}:" in
        *":/usr/libexec/apex/tools:"*) ;;
        *) PATH="/usr/libexec/apex/tools:${PATH}"; export PATH ;;
    esac
fi
