# Rime OS — fish completion for `rime`.
#
# Autoloaded by fish the first time somebody completes `rime`, because the file
# is named after the command and sits on $__fish_vendor_completionsdirs. Nothing
# here runs at shell startup.
#
# The lists that are a security boundary — requestable verbs, secret
# capabilities, agent adapters — are asked of the CLI rather than copied here.
# A completion list that drifts out of step with the vocabulary the daemon
# accepts offers operations it refuses, or stops offering one it accepts and
# makes it look unsupported. The helper functions live in
# /usr/share/fish/vendor_conf.d/rime-agent.fish.

# Position-aware predicates. `__fish_seen_subcommand_from` is not enough on its
# own here: `list` is a verb under agent, project, request AND secret, so a
# predicate that only asks "was the word `list` typed" fires in four places.
# These ask which POSITION a word is in.
function __rime_tokens --description 'the command line so far, as a list'
    commandline -opc
end

function __rime_at --description 'is the cursor at argument N'
    set -l t (__rime_tokens)
    test (count $t) -eq $argv[1]
end

function __rime_group --description 'is argument 1 this group'
    set -l t (__rime_tokens)
    test (count $t) -ge 2; and test "$t[2]" = "$argv[1]"
end

function __rime_verb --description 'is argument 1 this group and argument 2 this verb'
    set -l t (__rime_tokens)
    test (count $t) -ge 3; and test "$t[2]" = "$argv[1]"; and test "$t[3]" = "$argv[2]"
end

# No file completion anywhere by default. `rime` takes ids, names and verbs, and
# offering every file in the directory buries them.
complete -c rime -f

# ── top level ───────────────────────────────────────────────────────────────
complete -c rime -n '__rime_at 1' -a status -d 'what this machine is doing'
complete -c rime -n '__rime_at 1' -a tier -d 'performance tier'
complete -c rime -n '__rime_at 1' -a profile -d 'power profile'
complete -c rime -n '__rime_at 1' -a battery -d 'battery health and charge limit'
complete -c rime -n '__rime_at 1' -a fan -d 'fan curve'
complete -c rime -n '__rime_at 1' -a game -d 'per-game profiles'
complete -c rime -n '__rime_at 1' -a agent -d 'the agent runtime'
complete -c rime -n '__rime_at 1' -a project -d 'projects, worktrees and layouts'
complete -c rime -n '__rime_at 1' -a request -d 'privilege requests'
complete -c rime -n '__rime_at 1' -a fingerprint -d 'image fingerprint'
complete -c rime -n '__rime_at 1' -a pin -d 'pin the current deployment'
complete -c rime -n '__rime_at 1' -a rollback -d 'boot the previous deployment'
complete -c rime -n '__rime_at 1' -a update -d 'stage an image update'
complete -c rime -n '__rime_at 1' -a shell -d 'shell integration'
complete -c rime -n '__rime_at 1' -a metrics -d 'runtime metrics'
complete -c rime -n '__rime_at 1' -a doctor -d 'check this machine over'
complete -c rime -n '__rime_at 1' -a image -d 'image information'
complete -c rime -n '__rime_at 1' -a install -d 'install a package'
complete -c rime -n '__rime_at 1' -a remove -d 'remove a package'
complete -c rime -n '__rime_at 1' -a search -d 'search for a package'
complete -c rime -n '__rime_at 1' -a repo -d 'package repositories'
complete -c rime -n '__rime_at 1' -a pkg -d 'the package layer'

# ── rime agent ──────────────────────────────────────────────────────────────
complete -c rime -n '__rime_group agent; and __rime_at 2' -a run -d 'start an agent here'
complete -c rime -n '__rime_group agent; and __rime_at 2' -a list -d 'sessions'
complete -c rime -n '__rime_group agent; and __rime_at 2' -a attach -d 'reattach to a session'
complete -c rime -n '__rime_group agent; and __rime_at 2' -a input -d 'type text into a session'
complete -c rime -n '__rime_group agent; and __rime_at 2' -a rename -d 'name a session'
complete -c rime -n '__rime_group agent; and __rime_at 2' -a handoff -d 'hand a session to another agent'
complete -c rime -n '__rime_group agent; and __rime_at 2' -a pause -d 'stop a session'
complete -c rime -n '__rime_group agent; and __rime_at 2' -a resume -d 'continue a paused session'
complete -c rime -n '__rime_group agent; and __rime_at 2' -a kill -d 'end a session'
complete -c rime -n '__rime_group agent; and __rime_at 2' -a logs -d 'a session transcript'
complete -c rime -n '__rime_group agent; and __rime_at 2' -a status -d 'one session in detail'
complete -c rime -n '__rime_group agent; and __rime_at 2' -a default -d 'which agent `a` runs'
complete -c rime -n '__rime_group agent; and __rime_at 2' -a adapters -d 'installed agents'
complete -c rime -n '__rime_group agent; and __rime_at 2' -a diff -d 'what an agent changed'
complete -c rime -n '__rime_group agent; and __rime_at 2' -a undo -d 'roll a session back'
complete -c rime -n '__rime_group agent; and __rime_at 2' -a checkpoint -d 'checkpoints'
complete -c rime -n '__rime_group agent; and __rime_at 2' -a event -d 'report a state change'
complete -c rime -n '__rime_group agent; and __rime_at 2' -a rm -d 'forget a finished session'
complete -c rime -n '__rime_group agent; and __rime_at 2' -a prune -d 'forget every finished session'
complete -c rime -n '__rime_group agent; and __rime_at 2' -a enable -d 'turn the runtime on'

for v in attach input rename handoff pause resume kill logs rm status diff undo
    complete -c rime -n "__rime_verb agent $v" -a '(_rime_session_ids)' -d session
end
complete -c rime -n '__rime_verb agent rename' -l clear -d 'remove the name'
complete -c rime -n '__rime_verb agent run' -s n -l name -x -d 'what to call the session'
complete -c rime -n '__rime_verb agent input' -l submit -d 'press Enter after it'
complete -c rime -n '__rime_verb agent default' -a '(_rime_agent_names)' -d agent
complete -c rime -n '__rime_verb agent event' \
    -a 'working waiting_for_user permission_request complete failed' -d state

complete -c rime -n '__rime_group agent' -s a -l agent -x -a '(_rime_agent_names)' -d 'which agent'
complete -c rime -n '__rime_verb agent handoff' -s t -l to -x -a '(_rime_agent_names)' -d 'hand it to'
complete -c rime -n '__rime_group agent' -s s -l sandbox -x -a 'strict project unrestricted' -d 'confinement'

# ── rime project ────────────────────────────────────────────────────────────
complete -c rime -n '__rime_group project; and __rime_at 2' -a list -d 'known projects'
complete -c rime -n '__rime_group project; and __rime_at 2' -a info -d 'one project in detail'
complete -c rime -n '__rime_group project; and __rime_at 2' -a worktrees -d 'worktrees of this project'
complete -c rime -n '__rime_group project; and __rime_at 2' -a checkpoints -d 'checkpoints of this project'
complete -c rime -n '__rime_group project; and __rime_at 2' -a remove -d 'remove a worktree'
complete -c rime -n '__rime_group project; and __rime_at 2' -a forget -d 'stop tracking a project'
complete -c rime -n '__rime_group project; and __rime_at 2' -a env -d 'the capsule this project builds in'
complete -c rime -n '__rime_group project; and __rime_at 2' -a layout -d 'windows and terminals'
complete -c rime -n '__rime_group project; and __rime_at 2' -a switch -d 'go to a project'
complete -c rime -n '__rime_verb project layout; and __rime_at 3' \
    -a 'save show restore forget templates open' -d 'layout verb'
complete -c rime -n '__rime_verb project layout; and __rime_at 4' \
    -a '(_rime_layout_templates)' -d template

# ── rime request ────────────────────────────────────────────────────────────
complete -c rime -n '__rime_group request; and __rime_at 2' -a ask -d 'ask for a privileged operation'
complete -c rime -n '__rime_group request; and __rime_at 2' -a list -d 'requests'
complete -c rime -n '__rime_group request; and __rime_at 2' -a pending -d 'requests waiting on you'
complete -c rime -n '__rime_group request; and __rime_at 2' -a show -d 'one request in detail'
complete -c rime -n '__rime_group request; and __rime_at 2' -a approve -d 'allow a request'
complete -c rime -n '__rime_group request; and __rime_at 2' -a deny -d 'refuse a request'
complete -c rime -n '__rime_group request; and __rime_at 2' -a verbs -d 'what can be asked for'
complete -c rime -n '__rime_group request; and __rime_at 2' -a grants -d 'standing grants'
complete -c rime -n '__rime_group request; and __rime_at 2' -a revoke -d 'withdraw a grant'
complete -c rime -n '__rime_group request; and __rime_at 2' -a audit -d 'the decision log'
complete -c rime -n '__rime_verb request ask' -a '(_rime_request_verbs)' -d operation
for v in show approve deny
    complete -c rime -n "__rime_verb request $v" -a '(_rime_request_ids)' -d request
end

# ── rime secret ─────────────────────────────────────────────────────────────
complete -c rime -n '__rime_group secret; and __rime_at 2' -a add -d 'store a secret'
complete -c rime -n '__rime_group secret; and __rime_at 2' -a list -d 'stored services'
complete -c rime -n '__rime_group secret; and __rime_at 2' -a remove -d 'forget a secret'
complete -c rime -n '__rime_group secret; and __rime_at 2' -a capabilities -d 'what can be granted'
complete -c rime -n '__rime_group secret; and __rime_at 2' -a grant -d 'grant a capability'
complete -c rime -n '__rime_group secret; and __rime_at 2' -a revoke -d 'withdraw a capability'
complete -c rime -n '__rime_group secret; and __rime_at 2' -a grants -d 'standing grants'
complete -c rime -n '__rime_group secret; and __rime_at 2' -a use -d 'use a secret without seeing it'
complete -c rime -n '__rime_group secret; and __rime_at 2' -a audit -d 'the broker log'
for v in remove grant revoke use
    complete -c rime -n "__rime_verb secret $v; and __rime_at 3" -a '(_rime_secret_services)' -d service
    complete -c rime -n "__rime_verb secret $v; and __rime_at 4" -a '(_rime_secret_capabilities)' -d capability
end
