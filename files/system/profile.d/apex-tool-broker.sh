# APEX: put the brokered `wrangler` and `terraform` ahead of the real ones —
# but only inside an agent session.
#
# P1-012's criterion is that existing skills keep invoking normal tools. A
# skill runs `wrangler deploy`; this is what makes that command reach the
# broker, which runs the real wrangler with a credential the agent never holds.
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
