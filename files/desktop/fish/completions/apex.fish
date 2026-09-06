# APEX-OS — fish completion for `apex`.
#
# Autoloaded by fish the first time somebody completes `apex`, because the file
# is named after the command and sits on $__fish_vendor_completionsdirs. Nothing
# here runs at shell startup.
#
# The lists that are a security boundary — requestable verbs, secret
# capabilities, agent adapters — are asked of the CLI rather than copied here.
# A completion list that drifts out of step with the vocabulary the daemon
# accepts offers operations it refuses, or stops offering one it accepts and
# makes it look unsupported. The helper functions live in
# /usr/share/fish/vendor_conf.d/apex-agent.fish.

# Position-aware predicates. `__fish_seen_subcommand_from` is not enough on its
# own here: `list` is a verb under agent, project, request AND secret, so a
# predicate that only asks "was the word `list` typed" fires in four places.
# These ask which POSITION a word is in.
function __apex_tokens --description 'the command line so far, as a list'
    commandline -opc
end

function __apex_at --description 'is the cursor at argument N'
    set -l t (__apex_tokens)
    test (count $t) -eq $argv[1]
end

function __apex_group --description 'is argument 1 this group'
    set -l t (__apex_tokens)
    test (count $t) -ge 2; and test "$t[2]" = "$argv[1]"
end

function __apex_verb --description 'is argument 1 this group and argument 2 this verb'
    set -l t (__apex_tokens)
    test (count $t) -ge 3; and test "$t[2]" = "$argv[1]"; and test "$t[3]" = "$argv[2]"
end

# No file completion anywhere by default. `apex` takes ids, names and verbs, and
# offering every file in the directory buries them.
complete -c apex -f

# ── top level ───────────────────────────────────────────────────────────────
complete -c apex -n '__apex_at 1' -a status -d 'what this machine is doing'
complete -c apex -n '__apex_at 1' -a tier -d 'performance tier'
complete -c apex -n '__apex_at 1' -a profile -d 'power profile'
complete -c apex -n '__apex_at 1' -a battery -d 'battery health and charge limit'
complete -c apex -n '__apex_at 1' -a fan -d 'fan curve'
complete -c apex -n '__apex_at 1' -a game -d 'per-game profiles'
complete -c apex -n '__apex_at 1' -a agent -d 'the agent runtime'
complete -c apex -n '__apex_at 1' -a project -d 'projects, worktrees and layouts'
complete -c apex -n '__apex_at 1' -a request -d 'privilege requests'
complete -c apex -n '__apex_at 1' -a fingerprint -d 'image fingerprint'
complete -c apex -n '__apex_at 1' -a pin -d 'pin the current deployment'
complete -c apex -n '__apex_at 1' -a rollback -d 'boot the previous deployment'
complete -c apex -n '__apex_at 1' -a update -d 'stage an image update'
complete -c apex -n '__apex_at 1' -a shell -d 'shell integration'
complete -c apex -n '__apex_at 1' -a metrics -d 'runtime metrics'
complete -c apex -n '__apex_at 1' -a doctor -d 'check this machine over'
complete -c apex -n '__apex_at 1' -a image -d 'image information'
complete -c apex -n '__apex_at 1' -a install -d 'install a package'
complete -c apex -n '__apex_at 1' -a remove -d 'remove a package'
complete -c apex -n '__apex_at 1' -a search -d 'search for a package'
complete -c apex -n '__apex_at 1' -a repo -d 'package repositories'
complete -c apex -n '__apex_at 1' -a pkg -d 'the package layer'

# ── apex agent ──────────────────────────────────────────────────────────────
complete -c apex -n '__apex_group agent; and __apex_at 2' -a run -d 'start an agent here'
complete -c apex -n '__apex_group agent; and __apex_at 2' -a list -d 'sessions'
complete -c apex -n '__apex_group agent; and __apex_at 2' -a attach -d 'reattach to a session'
complete -c apex -n '__apex_group agent; and __apex_at 2' -a pause -d 'stop a session'
complete -c apex -n '__apex_group agent; and __apex_at 2' -a resume -d 'continue a paused session'
complete -c apex -n '__apex_group agent; and __apex_at 2' -a kill -d 'end a session'
complete -c apex -n '__apex_group agent; and __apex_at 2' -a logs -d 'a session transcript'
complete -c apex -n '__apex_group agent; and __apex_at 2' -a status -d 'one session in detail'
complete -c apex -n '__apex_group agent; and __apex_at 2' -a default -d 'which agent `a` runs'
complete -c apex -n '__apex_group agent; and __apex_at 2' -a adapters -d 'installed agents'
complete -c apex -n '__apex_group agent; and __apex_at 2' -a diff -d 'what an agent changed'
complete -c apex -n '__apex_group agent; and __apex_at 2' -a undo -d 'roll a session back'
complete -c apex -n '__apex_group agent; and __apex_at 2' -a checkpoint -d 'checkpoints'
complete -c apex -n '__apex_group agent; and __apex_at 2' -a event -d 'report a state change'
complete -c apex -n '__apex_group agent; and __apex_at 2' -a rm -d 'forget a finished session'
complete -c apex -n '__apex_group agent; and __apex_at 2' -a prune -d 'forget every finished session'
complete -c apex -n '__apex_group agent; and __apex_at 2' -a enable -d 'turn the runtime on'

for v in attach pause resume kill logs rm status diff undo
    complete -c apex -n "__apex_verb agent $v" -a '(_apex_session_ids)' -d session
end
complete -c apex -n '__apex_verb agent default' -a '(_apex_agent_names)' -d agent
complete -c apex -n '__apex_verb agent event' \
    -a 'working waiting_for_user permission_request complete failed' -d state

complete -c apex -n '__apex_group agent' -s a -l agent -x -a '(_apex_agent_names)' -d 'which agent'
complete -c apex -n '__apex_group agent' -s s -l sandbox -x -a 'strict project unrestricted' -d 'confinement'

# ── apex project ────────────────────────────────────────────────────────────
complete -c apex -n '__apex_group project; and __apex_at 2' -a list -d 'known projects'
complete -c apex -n '__apex_group project; and __apex_at 2' -a info -d 'one project in detail'
complete -c apex -n '__apex_group project; and __apex_at 2' -a worktrees -d 'worktrees of this project'
complete -c apex -n '__apex_group project; and __apex_at 2' -a checkpoints -d 'checkpoints of this project'
complete -c apex -n '__apex_group project; and __apex_at 2' -a remove -d 'remove a worktree'
complete -c apex -n '__apex_group project; and __apex_at 2' -a forget -d 'stop tracking a project'
complete -c apex -n '__apex_group project; and __apex_at 2' -a env -d 'the capsule this project builds in'
complete -c apex -n '__apex_group project; and __apex_at 2' -a layout -d 'windows and terminals'
complete -c apex -n '__apex_group project; and __apex_at 2' -a switch -d 'go to a project'
complete -c apex -n '__apex_verb project layout; and __apex_at 3' \
    -a 'save show restore forget templates open' -d 'layout verb'
complete -c apex -n '__apex_verb project layout; and __apex_at 4' \
    -a '(_apex_layout_templates)' -d template

# ── apex request ────────────────────────────────────────────────────────────
complete -c apex -n '__apex_group request; and __apex_at 2' -a ask -d 'ask for a privileged operation'
complete -c apex -n '__apex_group request; and __apex_at 2' -a list -d 'requests'
complete -c apex -n '__apex_group request; and __apex_at 2' -a pending -d 'requests waiting on you'
complete -c apex -n '__apex_group request; and __apex_at 2' -a show -d 'one request in detail'
complete -c apex -n '__apex_group request; and __apex_at 2' -a approve -d 'allow a request'
complete -c apex -n '__apex_group request; and __apex_at 2' -a deny -d 'refuse a request'
complete -c apex -n '__apex_group request; and __apex_at 2' -a verbs -d 'what can be asked for'
complete -c apex -n '__apex_group request; and __apex_at 2' -a grants -d 'standing grants'
complete -c apex -n '__apex_group request; and __apex_at 2' -a revoke -d 'withdraw a grant'
complete -c apex -n '__apex_group request; and __apex_at 2' -a audit -d 'the decision log'
complete -c apex -n '__apex_verb request ask' -a '(_apex_request_verbs)' -d operation
for v in show approve deny
    complete -c apex -n "__apex_verb request $v" -a '(_apex_request_ids)' -d request
end

# ── apex secret ─────────────────────────────────────────────────────────────
complete -c apex -n '__apex_group secret; and __apex_at 2' -a add -d 'store a secret'
complete -c apex -n '__apex_group secret; and __apex_at 2' -a list -d 'stored services'
complete -c apex -n '__apex_group secret; and __apex_at 2' -a remove -d 'forget a secret'
complete -c apex -n '__apex_group secret; and __apex_at 2' -a capabilities -d 'what can be granted'
complete -c apex -n '__apex_group secret; and __apex_at 2' -a grant -d 'grant a capability'
complete -c apex -n '__apex_group secret; and __apex_at 2' -a revoke -d 'withdraw a capability'
complete -c apex -n '__apex_group secret; and __apex_at 2' -a grants -d 'standing grants'
complete -c apex -n '__apex_group secret; and __apex_at 2' -a use -d 'use a secret without seeing it'
complete -c apex -n '__apex_group secret; and __apex_at 2' -a audit -d 'the broker log'
for v in remove grant revoke use
    complete -c apex -n "__apex_verb secret $v; and __apex_at 3" -a '(_apex_secret_services)' -d service
    complete -c apex -n "__apex_verb secret $v; and __apex_at 4" -a '(_apex_secret_capabilities)' -d capability
end
