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

## Project layouts

§6 asks APEX to remember the windows and terminals of a project and restore
them after a reboot.

```
apex project layout save              # capture what is open in this project
apex project layout show              # what would come back
apex project layout restore           # reopen it
apex project layout restore --dry-run # print, start nothing
apex project layout forget
```

### What is remembered

Not window handles. A Hyprland address and a niri window id are both
meaningless after a restart, so a layout naming them would be restorable
exactly zero times. What is stored is how to *recreate* each window: its argv,
its working directory, and the workspace it was on.

### Which windows belong to a project

Decided from the working directory of the process tree behind each window,
never from the title — a title is whatever the application chose to print, and
matching on it would capture an unrelated editor that happens to have the
project name on a tab.

The subtlety is that a terminal's own working directory is where it was
*launched*, usually `$HOME`; the shell inside it is what moved into the project.
So the resolver checks the window's process and then its descendants,
breadth-first, and takes the first directory under the project root. Breadth
first on purpose: the shell directly inside a terminal is the directory a user
thinks of as "where that window is", not whatever a nested build step last
`cd`-ed into.

A window with no pid is skipped. labwc reports none — it exposes no IPC and no
window-management protocol beyond the standard Wayland ones by design — so on
labwc `save` reports that it cannot match windows to a project rather than
guessing.

### Restoring

A terminal is *not* restored with its stored argv. That argv is typically the
bare emulator name, because it inherited its working directory from whatever
launched it, so replaying it opens a terminal in the wrong place — the most
useless possible outcome of "restore my project". Instead the working directory
is passed explicitly, with the flag that emulator actually uses (they all
differ, and a wrong flag is usually treated as a command to run, so the window
opens, fails and closes).

An application *is* restored verbatim, because its argv carries its own
arguments.

Restore is a command and not a login hook, deliberately: a session that reopens
fourteen windows nobody asked for is worse than one that reopens none.

### Switching by project

```
apex project switch            # this project
apex project switch apex-os    # by name, from anywhere
```

§6's "allow switching by project, not only by numeric workspace". It needs a
saved layout, because that is what records which workspace a project lives on —
a project does not own a workspace, it merely has windows that were on one.

Where a layout spans several workspaces the most populated one wins. That is a
choice rather than an obvious truth (the alternative is the first one captured),
and it is the one that matches what people mean by "where the project is".

Placement onto workspaces is best-effort. A window cannot be moved before it
exists, and it does not exist until its process has mapped a surface — which is
asynchronous and unbounded — so `restore` reports the intended split rather than
holding the terminal open for seconds guessing at startup times.

### It runs stored command lines

Worth being plain about: the layout file is a list of argv vectors that
`apex project restore` executes. It lives under `$XDG_STATE_HOME` at `0700` and
is written only by your own runtime. It is executed as an argv **vector**, never
through a shell, so nothing in a stored entry can be interpreted as a shell
metacharacter — there is no shell to interpret it.

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

## The secret broker

§4: *"Agents should be able to use credentials without receiving the raw
secret… Expose usage permission, not necessarily secret value."*

```
printf %s "$TOKEN" | apex secret add github --host github.com
apex secret grant github git-push        # per project
apex secret use github git-push origin   # run by the agent
apex secret audit
```

### Why the broker performs the operation

The obvious implementation is a git credential helper the sandbox can reach. It
does not work, and the reason is worth writing down: **git runs inside the
sandbox**, so whatever the helper prints is on git's stdin, inside the agent's
own namespace, readable by the agent. A credential helper hands over the token
by construction.

So the broker performs the operation instead. The agent asks for
`git-push origin`; `apex-agentd`, which runs *outside* the sandbox, runs the
push and returns git's output. The token only ever exists in the environment of
a process the agent cannot see — the sandbox uses `--unshare-pid`, so the
daemon's children are not in the agent's `/proc` at all.

That is a **namespace** boundary, not a privilege one. The daemon is
unprivileged and runs as the same user; what it has that the session does not is
a view of the filesystem and the process table. For "the agent must not learn
the token", that is exactly the boundary required.

### The agent cannot name a URL

`git-push` takes a remote **name**, and the daemon resolves it against the
repository's own configuration. Accepting a URL would let a session ask the
broker to push a branch to `https://attacker.example/` with your token attached
— and the broker would, because it was told to.

The remote's host is then checked against the credential's host, so a grant for
GitHub cannot push to GitLab. An `ssh://` remote is refused with an explanation:
a token is not how ssh authenticates, and the ssh-agent socket is masked with
`$XDG_RUNTIME_DIR` by design.

### Where the secret lives, and why not the keyring

A `0600` file inside a `0700` directory under `$XDG_STATE_HOME`. That path is
inside `$HOME`, which a confined session masks with a tmpfs, so a
`project`-policy agent cannot read it — asserted both in the sandbox unit tests
and end-to-end from inside a real session.

The keyring is supported (`--keyring`) but is **not** the default, and that was
decided by measurement rather than principle: `secret-tool store` on APEX blocks
on a `gcr-prompter` "Unlock Keyring" dialog — it hung until it was killed — and
`gnome-keyring-daemon` ships disabled. A broker an agent calls must never hang
and must never raise a prompt in front of somebody who is not watching. Every
keyring call is bounded by a timeout for the same reason.

An `unrestricted` session can read the file, as it can read everything else.
That is what the escape hatch means.

### Order of checks

Peer credentials → project → grant → remote name → remote host → **then** the
token is read. Every step before the last can refuse, so a refusal cannot leak
the credential through an error path. Output returned to the caller is scrubbed
of the token as well: git does not normally print credentials, but some error
messages include a `https://user:token@host/…` URL.

### What is not built

Only `git-push` and `git-fetch`. `gh`-style API capabilities (read issues,
create a PR) are a second vocabulary with a second validation surface, and are
not needed to demonstrate the property. Scoped-token *issuance* — asking GitHub
for a narrower token per task — is also not here; the broker uses the token it
was given.

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
| `$XDG_STATE_HOME/apex/agent/layouts/` | saved project window layouts |
| `$XDG_STATE_HOME/apex/agent/secrets/` | brokered credentials, `0600` each |
| `$XDG_STATE_HOME/apex/agent/secret-grants.json` | per-project capability grants |
| `$XDG_STATE_HOME/apex/agent/secret-audit.jsonl` | append-only capability audit |
| `$XDG_STATE_HOME/apex/agent/privilege-audit.jsonl` | append-only privilege audit |
| `$XDG_CONFIG_HOME/apex/agent.json` | default agent, sandbox, detach key |
| `/tmp/apex-agent/<id>/` | per-session scratch, removed with the session |

Transcripts are a record of your work and are readable only by you.

---

## Remote sessions

`--host` exists on three of these verbs, and each one means something
different:

| | what it does |
| --- | --- |
| `apex agent run --host <device>` | forwards the WHOLE invocation to that device's own `apex agent run` |
| `apex agent list --host <device>` | the sessions over there, not here |
| `apex agent attach --host <device>` | a view onto a session that keeps running there |

The run form forwards rather than reimplements. The remote applies its own
sandbox policy, its own default agent and its own checkpointing, because that
is where the agent actually runs — reconstructing those decisions locally
would be two implementations of one policy, and the wrong one would be the
local copy. `RunArgs::forward_argv` rebuilds the flags from the parsed struct
rather than from `std::env::args`, so a flag clap normalised is forwarded
normalised, and the three local-only flags (`--host`, `--remote-path`,
`--allow-dirty`) cannot leak into the remote command and make it dispatch
again.

`--remote-path` names the project directory on the far side when it is not the
same absolute path, and skips the same-repository check. `--allow-dirty` runs
despite uncommitted changes here; they are NOT sent, because the remote works
from its own checkout.

The id an attach takes is the REMOTE's, which is why the list form exists.
`apex task resume` deliberately passes `host: None`: a resume attaches to a
session on this machine, and continuing one elsewhere stays explicit.

Devices come from `apex host` (§20's trusted devices), and the ssh argv —
including the `--` before the destination and the per-argument quoting — is
owned there. `tests/test-apex-dispatch.sh` covers these forms.

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

- **Scoped-token issuance.** The broker uses the token it is given; it does not
  ask a provider for a narrower one per task. The brokering itself exists — see
  *The secret broker* — with `git-push` and `git-fetch` as its vocabulary.
- **`gh`-style API capabilities** (read issues, create a PR). A second
  vocabulary with a second validation surface.
- **Unattended execution of a granted request.** "Allow for project" means the
  next identical request needs no decision; it does not yet mean the operation
  runs with nobody present. That would need a privileged executor reachable
  from an agent's request, and minting a new root surface is not something to
  do casually. See below.
- **Terminal layouts** as a designed grid (§3's editor/agent split), and
  tmux/zellij integration. Restoring a project's windows now exists — see
  *Project layouts* — but choosing a layout template does not.
- **Fish and nushell** shell integration. Bash and zsh are covered, including
  the `a`/`aa`/`al`/`ad`/`aw`/`ap` shortcuts, completion, and the optional
  prompt indicator (`apex_agent_prompt`, which is fork-free: it reads the
  session records the daemon already writes, at about 0.25 ms per prompt).
- **Screenshots and drag-and-drop into an agent** (§3's clipboard section).
- **tmux and zellij integration**, and layout TEMPLATES (§3's editor/agent
  grid). Restoring a project's own windows exists; choosing a layout shape does
  not.
- **Test status and merge conflicts per worktree** in the Agent Center (§7).
  The worktree a session is on is shown; whether its tests pass is not.
- **Disposable environments** and capsules.
