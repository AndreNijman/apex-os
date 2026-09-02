# The APEX agent runtime

Coding agents as a first-class OS workload, without replacing them.

`claude`, `opencode`, `codex`, `gemini` and anything else you run keep working
exactly as they do today. APEX adds what sits underneath: the terminal they run
on, the confinement they run inside, and the project state around them.

Nothing here is enabled by default. Start it with:

```
systemctl --user enable --now apex-agentd
```

---

## What it is

Three pieces:

| piece | what it is | privilege |
|---|---|---|
| `apex-agentd` | per-user daemon owning PTYs, sandboxes and session state | none |
| `apex agent` / `apex project` | CLI client over its control socket | none |
| `a`, `aa`, `al`, `ad`, `aw`, `ap` | shell shortcuts | none |

`apex-agentd` is **unprivileged and never talks to `apexd`**. Agent
orchestration handles untrusted model output and spawns arbitrary user
programs; putting that in the privileged daemon would make the worst case a
system compromise instead of a user-session one. When a session eventually
needs a system change, it is your own `apex` invocation that makes the narrow
request over `org.apexos.Apexd1` — not a right this daemon holds.

```
claude / opencode / codex / gemini / any binary
        │  the real upstream process, unmodified, in a real PTY
        ▼
apex-agentd  ── unprivileged, per-user, systemd --user
        ├─ PTY + session lifecycle
        ├─ bubblewrap sandbox
        ├─ adapters
        ├─ projects + git worktrees
        └─ checkpoints
        ▲
        │  newline-delimited JSON on a 0600 Unix socket
apex agent … / APEX Shell
```

---

## Everyday use

```
a                              # the agent you chose, here
a "fix the failing tests"      # with an opening instruction
al                             # what is running
aa                             # reattach to it
ad                             # what it changed
```

The long forms:

```
apex agent run "upgrade to Qt 7" --checkpoint --worktree qt7
apex agent list --all
apex agent attach 4
apex agent pause 4 / resume 4 / kill 4
apex agent logs 4
apex agent diff 4
apex agent undo 4
apex agent default opencode
apex project info / worktrees / checkpoints
```

Pick the agent `a` runs once:

```
apex agent default claude
```

`apex agent adapters` lists what is known and what is actually installed.

### The PTY is the point

APEX creates the terminal, then execs the ordinary agent binary inside it. The
agent sees a normal terminal, so nothing about it has to change — and because
the *daemon* owns the terminal rather than your shell, closing the window does
not kill the work. Detach with **ctrl-]** and reattach later from anywhere.

Attaching replays the session's scrollback, so you get the screen back as it
was, then live output. Several terminals can attach to one session at once.

---

## The sandbox

Three policies. `project` is the default.

|  | `unrestricted` | `project` | `strict` |
|---|---|---|---|
| project files | rw | rw | rw |
| rest of `$HOME` | visible | **not present** | **not present** |
| `/usr`, `/etc` | rw as you | read-only | read-only |
| other processes | all | own PID namespace | own PID namespace |
| camera, microphone | yes | **no** | **no** |
| network | yes | yes | **no** |

Measured on APEX-OS 43, kernel 7.1.5, bubblewrap 0.11.0:

| property | outside | inside `project` |
|---|---|---|
| processes visible in `/proc` | 408 | 4 |
| `/dev/video*` nodes | 4 | 0 |
| `/dev/snd` nodes | 14 | 0 |
| `~/.ssh` readable | yes | no |
| project readable/writable | yes | yes |

### Default-deny, not a blocklist

`$HOME`, `/run` and `$XDG_RUNTIME_DIR` are replaced with empty tmpfs mounts and
only an explicit allowlist is bound back. `~/.ssh`, `~/.gnupg`, `~/.aws`,
browser profiles and the ssh-agent and gpg-agent sockets are unreachable because
*nothing bound them* — not because something listed them. A blocklist would be
a hole every time a tool invented a new credential store.

`/run` is masked for a specific reason. `--ro-bind / /` made
`/run/dbus/system_bus_socket` visible, and it is mode `0666`. `apexd` lives on
that bus, and its mutating methods are gated by polkit actions that ship
`allow_active = yes` — passwordless for the logged-in local user. A confined
session *is* that user, so `SetTier`, `SetChargeThresholds`, `Fan.SetPwm` and
`GameMode.StartForPid` were all reachable from inside the sandbox. Measured, not
theorised: `SetTier` returned success from confinement.

A denylist of known sockets could not fix that. `/run` is a tmpfs on the host
and `--ro-bind / /` binds the same filesystem, so a socket created *after* the
sandbox starts appears inside it — anything computed at spawn time is stale by
construction. The one thing bound back is the `/etc/resolv.conf` link target,
read-only, without which every session loses DNS.

The environment works the same way: cleared, then rebuilt from locale, terminal
identity, and the specific variables the chosen adapter declares. A
`GITHUB_TOKEN` or `AWS_SECRET_ACCESS_KEY` in your shell does not reach a session
that never asked for it.

A few directories a toolchain genuinely needs are bound back writable
(`~/.cargo`, `~/.npm`, the Go module cache…), and the credential files that
happen to live inside them (`~/.cargo/credentials.toml`, `~/.npmrc`) are
blanked out again afterwards.

### It fails closed

If `bwrap` is missing, or the kernel has `dev.tty.legacy_tiocsti` enabled, a
confined session **does not start**. It is never silently downgraded to a weaker
policy than you asked for. The error names the escape hatch:

```
apex agent run --sandbox unrestricted …
```

### Known limits

- Escaping the sandbox is not in scope for the threat model. This confines a
  *cooperating but fallible* agent — one that follows a bad instruction or
  makes a mistake — not a determined kernel-exploit attacker.
- `unrestricted` confines nothing. That is deliberate; it is the escape hatch.
- Ordinary terminal processes are never sandboxed. Policy applies to sessions
  the runtime manages and to nothing else.
- Wayland and D-Bus session sockets are masked with the rest of
  `$XDG_RUNTIME_DIR`, so a confined agent cannot open GUI applications. The
  *system* bus is masked with `/run`, so it cannot reach `apexd` either — a
  system change has to go through `apex request` (below).

---

## Projects, worktrees and checkpoints

A project is a git working tree the runtime has seen. `apex project list` shows
them by recency; `apex project info` describes the current one.

### Parallel work

```
apex agent run "fix issue 217" --worktree issue-217
apex agent run "fix issue 221" --worktree issue-221
```

Each gets its own git worktree under `.apex/worktrees/` on branch
`agent/<name>`, so two agents never fight over one checkout. The directory is
ignored via `.git/info/exclude` rather than `.gitignore` — it is this machine's
runtime state, not something to commit and push to your colleagues.

Re-running with the same name reattaches to the same worktree.

### Undo

```
apex agent run "upgrade to Qt 7" --checkpoint
apex agent undo
```

A checkpoint captures tracked **and untracked** files as a real git tree, plus
`HEAD`, the branch, and your installed package list. Undo restores the working
tree, deletes files created since, and unwinds commits the agent made.

Specifically:

- Capture runs entirely through plumbing against a temporary index, so your
  staged changes, your stash and your branch are untouched.
- Undo takes a safety checkpoint **first**, so the undo is itself undoable.
- Checkpoints live under `refs/apex/checkpoints/`, not `refs/heads/`, so they
  never show up as branches and a plain `git push` never sends them.

Two deliberate boundaries:

- **Ignored files are not captured.** `.gitignore` exists to name build output
  and local secrets; sweeping a 4 GB `target/` and your `.env` into a git object
  is not an undo feature.
- **Packages are recorded, not removed.** Undo reports what was installed since
  the checkpoint and prints the `apex remove` line. Running a privileged,
  system-wide removal because you undid a working tree is not a call this makes
  for you.

---

## Status, and the open event protocol

`apex agent list` shows each session as `working`, `waiting_for_user`,
`permission_request`, `complete` or `failed`.

Most of that is inferred from the terminal: a bell, an OSC 9 / OSC 777 desktop
notification, OSC 133 prompt markers, silence past ten seconds, and the exit
status. Nothing scrapes pixels and nothing pattern-matches an agent's prose.

**`permission_request` is never guessed.** There is no reliable way to recognise
a permission prompt in arbitrary terminal output, and a wrong guess is worse
than none — it would report an agent as blocked while it works, or the reverse.
It is only ever set by a published event.

Any process inside a session can publish its own state:

```
apex agent event working
apex agent event permission_request --detail "wants to push a branch"
apex agent event complete
```

The session id comes from `$APEX_AGENT_SESSION`, which the runtime sets in every
session, so a hook script needs no arguments. That is the whole protocol: an
agent with hooks can wire them straight to it, and one without still gets the
inferred states.

---

## Privilege requests

An agent has no sudo, no root shell, and a sandbox that cannot reach the system
bus. When it genuinely needs a system change, it asks:

```
apex request install clang --reason "Required to compile the project"
```

and blocks. You see it, and decide:

```
apex request pending
sudo apex request approve 3                 # allow once, and run it
sudo apex request approve 3 --for-project   # …and stop asking for this one
apex request deny 3
```

### The vocabulary is closed

`apex request verbs` lists everything askable: `install`, `remove`,
`pkg-upgrade`, `pkg-rebuild`, `pkg-rollback`, `pin`, `rollback`, `update`. Each
maps to an `apex` subcommand that already declares itself root-only.

There is deliberately **no verb for running a command**. An `exec` variant would
be sudo with a confirmation dialog: nobody can meaningfully review an arbitrary
shell line, and approving `sh -c '…'` once is equivalent to granting permanent
root. The request type is a Rust enum, so this is a property of the type and not
of a validation function somebody can bypass.

Package names are checked against rpm's own rule, which excludes `/`, a leading
`-`, and every control character. That is not politeness — the approval prompt
is what you read to decide, so a name able to embed a newline or an escape
sequence could show you an operation other than the one being requested.

### Who is asking

The daemon resolves the asking session from the connection's **peer
credentials** (`SO_PEERCRED`), then walks that pid's `/proc` parent chain until
it meets a pid the daemon itself recorded when it forked a session.

It never reads `$APEX_AGENT_SESSION`. That variable is set inside each session
and is fine for `apex agent event`, where the worst a lying client achieves is a
wrong status label — but anything *authorised* by a client-supplied id is
authorised by the agent itself. Ancestry rather than process group, because a
process may `setpgid` itself and cannot choose its parent.

Consequently a session cannot approve its own request, cannot deny it, and
cannot alter its own grants. `tests/test-privilege-requests.sh` asserts all
three against a real daemon, and its negative control is a session that files a
request while claiming `APEX_AGENT_SESSION=99999` and is still attributed
correctly.

### Where the privilege comes from

`apex-agentd` is unprivileged and stays that way — §2's rule is that agent
orchestration must not live inside the privileged daemon. The daemon records,
validates and remembers; it never executes. The operation runs inside
`apex request approve`, under the same root gate as `apex install` itself, so
the privilege exercised is **yours**.

That is why a grant does not yet mean unattended execution: with nobody
present there is no privilege to borrow. Closing that gap means a privileged
executor reachable from an agent's request, and that is a new root surface — so
it is named in *Not implemented* rather than quietly added.

### The audit trail

Every filing, decision and execution appends one JSON line to
`privilege-audit.jsonl`, which is never rewritten:

```
apex request audit
```

The `argv` recorded is rebuilt from the typed verb, not stored as a string, so a
hand-edited request file cannot smuggle an extra argument in between the
approval and the execution.

---

## Files

| path | what |
|---|---|
| `$XDG_RUNTIME_DIR/apex-agentd/control.sock` | control socket, `0600` |
| `$XDG_STATE_HOME/apex/agent/sessions/` | session records |
| `$XDG_STATE_HOME/apex/agent/logs/` | transcripts, `0600`, capped at 32 MiB |
| `$XDG_STATE_HOME/apex/agent/checkpoints/` | checkpoint metadata |
| `$XDG_STATE_HOME/apex/agent/requests/` | privilege requests, one JSON file each |
| `$XDG_STATE_HOME/apex/agent/grants.json` | per-project "allow for project" grants |
| `$XDG_STATE_HOME/apex/agent/privilege-audit.jsonl` | append-only privilege audit |
| `$XDG_CONFIG_HOME/apex/agent.json` | default agent, sandbox, detach key |
| `/tmp/apex-agent/<id>/` | per-session scratch, removed with the session |

Transcripts are a record of your work and are readable only by you.

---

## Escape hatches

By design, none of this is compulsory:

- Run `claude`, `opencode`, `codex` or `gemini` directly. Nothing changes.
- `APEX_NO_AGENT_ALIASES=1` drops the shortcuts and keeps completion.
- `apex agent default` picks any adapter; `--agent generic` runs any binary.
- `--sandbox unrestricted` turns confinement off.
- The daemon is opt-in and `systemctl --user disable apex-agentd` ends it.

---

## Not implemented

Named because the roadmap asks for them and this does not do them:

- **Secret broker.** Credentials are passed as environment variables the adapter
  declares, not brokered as capabilities. Scoped-token issuance is not here.
- **Unattended execution of a granted request.** "Allow for project" means the
  next identical request needs no decision; it does not yet mean the operation
  runs with nobody present. That would need a privileged executor reachable
  from an agent's request, and minting a new root surface is not something to
  do casually. See below.
- **Terminal layouts**, tmux/zellij integration, and restoring a project's
  windows after reboot.
- **Remote sessions.** `--host` does not exist.
- **Fish and nushell** shell integration. Bash and zsh are covered.
- **Disposable environments** and capsules.
