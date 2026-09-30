# Rime OS — agent shell integration for nushell.
#
# The nushell half of files/desktop/shell/agent.sh, and a separate file for the
# same reason the fish one is: nushell is not a POSIX shell and cannot source
# that script.
#
# Installed to /usr/share/nushell/vendor/autoload/, which nushell names itself —
# `nu -c '$nu.vendor-autoload-dirs'` lists it first. Files there are sourced for
# every interactive session with no dotfile edited. That is also the limit of
# what an OS can do here: nushell reads vendor autoload files in the REPL only,
# so `nu -c '…'` and `nu script.nu` do not see them. The tests source this file
# explicitly for that reason, and assert separately that the install directory
# is one nushell actually reads.
#
# ── Two nushell facts this file is shaped by ────────────────────────────────
#
# `def`, `alias` and `extern` are parse-time declarations. Putting them inside
# an `if` does not conditionally define them — it defines nothing at all,
# because the block has its own scope. So there is no RIME_NO_AGENT_ALIASES
# guard here and no `command -v rime` guard: neither can be written. The opt-out
# is nushell's own, and it is documented rather than faked:
#
#     # ~/.config/nushell/config.nu
#     hide a; hide aa; hide al; hide ad; hide aw; hide ap
#
# `extern` is a SIGNATURE for an external command, not a wrapper. An unknown
# flag or an extra argument is passed straight through to `rime`, so these
# declarations add completion without ever refusing a command the CLI accepts —
# verified, because an `extern` that rejected arguments would be strictly worse
# than no completion at all.

# ── completion sources ──────────────────────────────────────────────────────
#
# Every one of these is silent on failure. Completion runs while somebody is
# typing; it must never print an error or hang because the runtime is not
# running. `complete` captures the external command rather than letting a
# non-zero exit propagate out of the closure.

def "nu-complete rime sessions" [] {
    let r = (^rime agent list --all --json | complete)
    if $r.exit_code != 0 { return [] }
    try { $r.stdout | from json | get id | each { |i| $i | into string } } catch { [] }
}

def "nu-complete rime running" [] {
    let r = (^rime agent list --json | complete)
    if $r.exit_code != 0 { return [] }
    try { $r.stdout | from json | get id | each { |i| $i | into string } } catch { [] }
}

def "nu-complete rime agents" [] {
    let r = (^rime agent adapters | complete)
    if $r.exit_code != 0 { return [] }
    $r.stdout | lines | skip 1 | each { |l| $l | split row -r '\s+' | get 0? | default "" }
        | each { |n| $n | str replace -a "*" "" } | where { |n| $n != "" }
}

# Stored secret services, and the capabilities that can be granted. Both asked
# of the CLI: the capability set is a security boundary, and a stale copy of it
# in a completion list misrepresents what the broker accepts.
def "nu-complete rime services" [] {
    let r = (^rime secret list --json | complete)
    if $r.exit_code != 0 { return [] }
    try { $r.stdout | from json | get service } catch { [] }
}

def "nu-complete rime capabilities" [] {
    let r = (^rime secret capabilities | complete)
    if $r.exit_code != 0 { return [] }
    $r.stdout | lines | where { |l| $l starts-with "  " } | each { |l| $l | str trim | split row " " | get 0 }
}

def "nu-complete rime requests" [] {
    let r = (^rime request list --all --json | complete)
    if $r.exit_code != 0 { return [] }
    try { $r.stdout | from json | get id | each { |i| $i | into string } } catch { [] }
}

# The requestable verbs, asked of the CLI rather than duplicated here. The
# vocabulary is a security boundary, so a completion list that drifts out of
# step with it would offer operations the daemon refuses — or, worse, stop
# offering one it accepts and make it look unsupported.
def "nu-complete rime operations" [] {
    let r = (^rime request verbs | complete)
    if $r.exit_code != 0 { return [] }
    $r.stdout | lines | where { |l| $l starts-with "  " } | each { |l| $l | str trim | split row " " | get 0 }
}

def "nu-complete rime sandbox" [] { ["strict" "project" "unrestricted"] }
def "nu-complete rime states" [] {
    ["working" "waiting_for_user" "permission_request" "complete" "failed"]
}

# ── the shortcuts ───────────────────────────────────────────────────────────

# Start an agent here. `a` with no arguments opens the agent interactively;
# `a "fix the tests"` gives it an opening instruction. An alias rather than a
# `def`, so flags reach `rime` untouched and `a --help` is rime's own help.
export alias a = ^rime agent run

export alias al = ^rime agent list
export alias ad = ^rime agent diff
export alias ap = ^rime project

# Reattach. `aa` with no id attaches to the only running session, which is the
# common case; with several it lists them rather than guessing. That decision
# needs a body, so this one is a command — `--wrapped`, so an id, a flag or both
# are forwarded verbatim.
export def --wrapped aa [...rest] {
    if ($rest | is-not-empty) {
        ^rime agent attach ...$rest
        return
    }
    let ids = (nu-complete rime running)
    if ($ids | length) != 1 {
        ^rime agent list
        return
    }
    ^rime agent attach ($ids | first)
}

# Whether a `rime agent run` command line already names its session, so `aw`
# does not name it twice (`--name` given twice is refused). Stops at `--`: what
# follows belongs to the agent binary.
def "rime names itself" [args: list<string>] {
    if ($env.RIME_AGENT_NAME? | default "" | str trim | is-not-empty) { return true }
    for arg in $args {
        if $arg == "--" { return false }
        if ($arg == "--name") or ($arg | str starts-with "--name=") or ($arg | str starts-with "-n") {
            return true
        }
    }
    false
}

# Start an agent in a worktree. The worktree's name is the session's name too,
# unless the command line or $RIME_AGENT_NAME already gives one; past the
# 64-character limit it is left off rather than passed along to be refused.
# The same rule as agent.sh.
export def --wrapped aw [...rest] {
    if ($rest | is-empty) {
        print -e "usage: aw <worktree-name> [prompt]"
        return
    }
    let wt = ($rest | first)
    let args = ($rest | skip 1)
    if (rime names itself $args) or (($wt | str length) > 64) {
        ^rime agent run --worktree $wt ...$args
    } else {
        ^rime agent run --worktree $wt --name $wt ...$args
    }
}

# ── prompt indicator ────────────────────────────────────────────────────────
#
# Opt-in, and FORK-FREE: it runs before every prompt, and a prompt that costs
# three processes per command is a prompt people turn off. Everything here is a
# nushell builtin — `glob`, `open`, `from json`, `path` — so nothing is spawned.
#
# Relevance is decided from $env.PWD against each session's recorded project
# root, so there is no need to ask git where we are.
#
# Usage — add to ~/.config/nushell/config.nu:
#     $env.PROMPT_COMMAND = {|| $"(rime-agent-prompt)(pwd)" }
export def rime-agent-prompt []: nothing -> string {
    let base = ($env.XDG_STATE_HOME? | default $"($env.HOME)/.local/state")
    let dir = ([$base "rime" "agent" "sessions"] | path join)
    if not ($dir | path exists) { return "" }

    mut working = 0
    mut waiting = 0
    mut attention = 0

    for f in (glob $"($dir)/*.json") {
        let rec = (try { open --raw $f | from json } catch { null })
        if $rec == null { continue }

        # Only sessions whose project contains $PWD. A session with no project
        # is skipped rather than shown everywhere.
        let root = ($rec.project? | default "")
        if ($root | is-empty) { continue }
        if not (($env.PWD == $root) or ($env.PWD | str starts-with $"($root)/")) { continue }

        # A finished session is not worth a prompt indicator.
        if ($rec.exit_code? | default null) != null { continue }

        match ($rec.state? | default "") {
            "working" => { $working = $working + 1 }
            "waiting_for_user" => { $waiting = $waiting + 1 }
            "permission_request" => { $attention = $attention + 1 }
            _ => {}
        }
    }

    mut out = ""
    if $working > 0 { $out = $"($out)󰜎($working)" }
    if $waiting > 0 { $out = $"($out)󰅺($waiting)" }
    if $attention > 0 { $out = $"($out)󰌾($attention)" }
    if ($out | is-empty) { "" } else { $"($out) " }
}

# ── completion ──────────────────────────────────────────────────────────────
#
# Declared per leaf subcommand. nushell routes an external invocation to the
# longest matching `extern`, so `rime agent attach 4` finds the one below while
# `rime agent adapters` — which has none — stays a plain external call.

export extern "rime agent attach" [
    id?: string@"nu-complete rime sessions"
]
export extern "rime agent status" [ id?: string@"nu-complete rime sessions" ]
export extern "rime agent pause"  [ id?: string@"nu-complete rime sessions" ]
export extern "rime agent resume" [ id?: string@"nu-complete rime sessions" ]
export extern "rime agent kill"   [ id?: string@"nu-complete rime sessions" ]
export extern "rime agent logs"   [ id?: string@"nu-complete rime sessions" ]
export extern "rime agent rm"     [ id?: string@"nu-complete rime sessions" ]
export extern "rime agent diff"   [ id?: string@"nu-complete rime sessions" ]
export extern "rime agent undo"   [ id?: string@"nu-complete rime sessions" ]
export extern "rime agent default" [ agent?: string@"nu-complete rime agents" ]
export extern "rime agent event"  [ state?: string@"nu-complete rime states" ]
export extern "rime agent run" [
    prompt?: string
    --agent(-a): string@"nu-complete rime agents"
    --name(-n): string
    --sandbox(-s): string@"nu-complete rime sandbox"
    --worktree(-w): string
    --checkpoint(-c): string
    --detach
]
export extern "rime agent rename" [
    id?: string@"nu-complete rime sessions"
    ...name: string
    --clear
    --host: string
]

export extern "rime request ask"     [ operation?: string@"nu-complete rime operations" ]
export extern "rime request show"    [ id?: string@"nu-complete rime requests" ]
export extern "rime request approve" [ id?: string@"nu-complete rime requests" ]
export extern "rime request deny"    [ id?: string@"nu-complete rime requests" ]

export extern "rime secret remove" [ service?: string@"nu-complete rime services" ]
export extern "rime secret grant" [
    service?: string@"nu-complete rime services"
    capability?: string@"nu-complete rime capabilities"
]
export extern "rime secret revoke" [
    service?: string@"nu-complete rime services"
    capability?: string@"nu-complete rime capabilities"
]
export extern "rime secret use" [
    service?: string@"nu-complete rime services"
    capability?: string@"nu-complete rime capabilities"
]

def "nu-complete rime layout" [] {
    ["save" "show" "restore" "forget" "templates" "open"]
}

# Templates, asked of the CLI. A hardcoded list would go stale the moment one is
# added, and offering a template that does not exist teaches a command that
# fails.
def "nu-complete rime templates" [] {
    let r = (^rime project layout templates | complete)
    if $r.exit_code != 0 { return [] }
    $r.stdout | lines | skip 1 | each { |l| $l | split row -r '\s+' | get 0? | default "" }
        | where { |n| $n != "" }
}

export extern "rime project layout" [ verb?: string@"nu-complete rime layout" ]
export extern "rime project layout open" [
    template?: string@"nu-complete rime templates"
    --mux: string
    --agents: int
    --dry-run
]
