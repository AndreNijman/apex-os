# The APEX agent runtime

Coding agents as a first-class OS workload, without replacing them.

`claude`, `opencode`, `codex`, `gemini` and anything else you run keep working
exactly as they do today. APEX adds what sits underneath: the terminal they run
on, the confinement they run inside, and the project state around them.

Nothing here is enabled by default. Turn it on with:

```
apex agent enable
```

That works for any user, root included. As root it first gives root a lingering
systemd user instance (root has none by default), then enables the service and
prints that agents started there run as root: the per-session sandbox keeps `/`
read-only and masks the home, but the working directory is writable, so a normal
user account is safer.

---

## What it is

Three pieces:

| piece | what it is | privilege |
|---|---|---|
| `apex-agentd` | per-user daemon owning PTYs, sandboxes and session state | none |
| `apex-secretd` | system daemon owning brokered credentials | root |
| `apex agent` / `apex project` | CLI client over its control socket | none |
| `a`, `aa`, `al`, `ad`, `aw`, `ap` | shell shortcuts | none |

`apex-agentd` is **unprivileged and never talks to `apexd`**. Agent
orchestration handles untrusted model output and spawns arbitrary user
programs; putting that in the privileged daemon would make the worst case a
system compromise instead of a user-session one. When a session eventually
needs a system change, it is your own `apex` invocation that makes the narrow
request over `org.apexos.Apexd1` — not a right this daemon holds.

`apex-secretd` is the one privileged piece, and it is a separate daemon for
that reason. It holds credentials and nothing else, it has no verb that returns
one, and the agent runtime is one of its clients rather than its owner. See
*The secret service* below.

```
claude / opencode / codex / gemini / any binary
        │  the real upstream process, unmodified, in a real PTY
        ▼
apex-agentd  ── unprivileged, per-user, systemd --user
        ├─ PTY + session lifecycle
        ├─ bubblewrap sandbox
        ├─ adapters
        ├─ projects + git worktrees
        ├─ checkpoints
        └─ capability requests ──┐
        ▲                        │  newline-delimited JSON on /run/apex-secretd
        │                        ▼
        │              apex-secretd  ── root, system service
        │                        ├─ the store, /var/lib/apex-secretd, 0700
        │                        └─ git, run as the owner with the credential
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
apex agent allow api.example.com
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

## Six permission dimensions

§3.1 names six controls that must never become one switch, and each is a
separate flag with a separate default:

| # | dimension | flag | values | default |
|---|---|---|---|---|
| 1 | the agent's own permission mode | `--native` | `inherit` `ask` `bypass` | `inherit` |
| 2 | APEX filesystem/process sandbox | `--sandbox` | `unrestricted` `project` `strict` | `project` |
| 3 | APEX system/root capability | `--system-access` | `none` `session` `unsafe` | `none` |
| 4 | APEX secret capability | `--secrets` | `brokered` `none` `export` | `brokered` |
| 5 | network policy | `--network` | `open` `allowlist` `brokered` `offline` | `open` |
| 6 | remote-origin policy | `--origin-policy` | `local` `remote` | `local` |

`apex agent status <id>` prints all six for a session, and `apex agent status`
with no id prints the configured defaults — six sibling keys in `agent.json`.

### The named modes are presets over the six

§4's modes are points in that space, not a seventh setting. `apex agent run`
takes the preset first and your own flags on top, so a combination none of the
five names stays reachable — break-glass with the broker switched off, say.

| mode | native | sandbox | system | secrets | network | origin |
|---|---|---|---|---|---|---|
| default (§4.1) | inherit | project | none | brokered | open | local |
| `--agent-bypass` (§4.2) | **bypass** | project | none | brokered | open | local |
| `--sandbox unrestricted` (§4.3) | inherit | **unrestricted** | none | brokered | open | local |
| `--system-access` (§4.4) | **bypass** | **unrestricted** | **session** | brokered | open | local |
| `--unsafe-everything` (§4.5) | **bypass** | **unrestricted** | **unsafe** | brokered | open | local |

The secret column does not move, even for break-glass. §3.4: *"broker secrets
are still not conveniently dumped into the agent environment."*

### The three invariants

`apex-agent-core/tests/policy_invariants.rs` is these sentences as tests, each
asserted over the whole value set of the dimension that drives it:

- **`bypassPermissions` does not disable the APEX sandbox.** Dimension 1 is a
  flag handed to `claude`. It cannot reach the mount namespace the sandbox is
  built out of, and the test asserts that twice: once on the policy, once on the
  `bwrap` argv the policy produces.
- **Unrestricted user does not imply root.** No sandbox value moves dimension 3.
  On top of that, a managed session runs with `PR_SET_NO_NEW_PRIVS`, so `sudo`,
  `su` and `pkexec` come up unprivileged inside it and fail. `bwrap` set that
  for confined sessions already; the runtime now sets it for the unconfined
  ones, which is §4.3's request.
- **A root grant does not imply secret export.** No system-access value moves
  dimension 4.

### Two dimensions do talk, and only downward

`strict` forces the network dimension to `offline`, because that is what
`strict` has always meant — so `--sandbox strict --network offline` and
`--sandbox project --network offline` build the same argv, and
`--sandbox strict --network open` is refused rather than quietly tightened.
Nothing loosens: `unrestricted` does not imply an open network.

`--sandbox unrestricted` with any network mode but `open` is refused. Each of
the other three is enforced by unsharing the session's network namespace, and
an unconfined session has none to unshare, so it would run with a network while
reporting none.

### What is refused until it is built

Four values parse and are then refused, each naming the task that will
implement it: `--system-access session`, `--system-access unsafe`,
`--secrets export`, `--origin-policy remote`. A flag that parsed and then did
nothing would read as a protection in `apex agent status` and in a script, with
nothing behind it.

---

## Network modes

Four, and three of them are the same kernel fact. `bwrap --unshare-net` gives
the session a namespace with nothing in it but loopback: no route, no resolver,
no addresses. What differs is what `apex-agentd` offers on the far side of a
Unix socket afterwards — `AF_UNIX` is a filesystem object, and a network
namespace does not touch it.

| mode | IP egress | what reaches the network for it | measured |
|---|---|---|---|
| `open` | everything | the session itself | `curl https://example.com` → 200 |
| `allowlist` | none | the egress proxy, for named destinations | allowed host → 200, other host → 403 |
| `brokered` | none | the capability broker, for named operations | `apex secret grants` answers, `curl` cannot resolve |
| `offline` | none | nothing | `curl` cannot resolve |

### `brokered`

`--unshare-net` plus the broker. The daemon runs the operation, outside the
namespace, and returns its result; the credential never enters the session.
This is how `git push` already works from a `strict` session, and it is the
mode a cloud provider's operations are meant to be used from — `Capability` in
`apex-agent-core/src/secret.rs` is the slot a provider adds to.

`--network brokered --secrets none` is refused. The broker is the session's
only way out and `--secrets none` is what shuts it, so the pair is an offline
session under another name.

### `allowlist`

`--unshare-net` plus one route back:

```text
inside the namespace                      outside it

  agent  ──HTTP CONNECT──▶  bridge  ──▶  socket  ──▶  apex-agentd  ──▶  internet
         127.0.0.1:3128    (no policy)   AF_UNIX      (decides)
```

The bridge is `apex-agentd` re-executed with `--net-bridge`, running as the
session's parent process. It has to be inside the sandbox because the loopback
an HTTP client can reach is the session's own. It carries bytes and holds no
policy, so replacing it gains an agent nothing: the far end is still the daemon.

`HTTPS_PROXY` and its five spellings are set so a client can find the bridge,
but they are not the enforcement. A session that unsets all six does not get a
direct connection — it gets `Could not resolve host`, measured.

**Destinations** live in `agent.json` as `network_allow`, managed with
`apex agent allow`. One `host` or `host:port` per entry; no port means 443 and
nothing else; `*.example.com` covers subdomains and not `example.com`. `*.com`
and `*` are refused. One unreadable entry empties the whole list and says
which, so the mode then refuses to start rather than running one line shorter
than it looks. An empty list is refused for the same reason.

The daemon checks the name, resolves it, checks every address that came back,
and connects to an address it checked — handing the name back to `connect()`
would resolve it twice, and the second answer is the one an attacker chooses.
An address on this machine or its LAN is refused unless a rule wrote that exact
address down: `localtest.me` is a public name that resolves to `127.0.0.1`, and
allowing it by name still does not reach anything, measured.

**What it does not stop.** Only proxy-aware HTTPS goes through it: there is no
resolver in the namespace, so `ssh`, raw TCP and UDP do not work at all. A
`CONNECT` tunnel is opaque, so a session allowed to reach a host may send it
anything, in any volume — this is a destination policy, not a data-loss one.
And a name on the list is only as trustworthy as its DNS; the local-address
guard covers the case that matters here, and the client's own TLS validation is
the rest.

Both decisions are pure functions in `apex-agent-core/src/destination.rs`, so
the whole table is asserted without a network.

---

## The sandbox

Three policies. `project` is the default.

|  | `unrestricted` | `project` | `strict` |
|---|---|---|---|
| project files | rw | rw | rw |
| rest of `$HOME` | visible | **not present** | **not present** |
| the agent's own profile | rw | **reusable half read-only** | **reusable half read-only** |
| `/usr`, `/etc` | rw as you | read-only | read-only |
| other processes | all | own PID namespace | own PID namespace |
| camera, microphone | yes | **no** | **no** |
| network | yes | follows `--network` | **no** |

The network row is the one the sandbox does not own: `strict` is `project` with
the network dimension forced to `offline`, and a `project` session gets
whatever `--network` says. See **Network modes** above.

Measured on APEX-OS 43, kernel 7.1.5, bubblewrap 0.11.0:

| property | outside | inside `project` |
|---|---|---|
| processes visible in `/proc` | 408 | 4 |
| `/dev/video*` nodes | 4 | 0 |
| `/dev/snd` nodes | 14 | 0 |
| `~/.ssh` readable | yes | no |
| project readable/writable | yes | yes |
| `~/.claude/skills` writable | yes | **no** |
| `~/.claude/CLAUDE.md` writable | yes | **no** |
| `~/.claude/projects` writable | yes | yes |

The last three are the agent profile, below.

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

## The agent profile

An agent installation is a profile, not just a binary. `claude` is an
executable plus a directory of instructions, skills, slash commands, plugins
and MCP definitions that decides what the executable does — which is why two
machines on the same version behave differently.

```bash
apex agent profile list
apex agent profile inspect claude
apex agent profile doctor claude
apex agent profile export claude --to ~/claude-profile
apex agent profile sync claude --from ~/claude-profile
```

### Reusable, machine-local, mixed, secret

Every part of the profile has a class, and the class decides both what an
export carries and how a confined session mounts it. One table, so the two
answers cannot drift apart.

| class | what it is | exported | mounted |
|---|---|---|---|
| reusable | instructions, skills, commands, subagents | whole | read-only |
| mixed | reusable and machine-local in one file | in part | read-only, except `~/.claude.json` |
| machine-local | transcripts, caches, plugin state, install paths | no | writable |
| secret | credentials | **never** | writable |

**Anything the table does not name is machine-local.** That is what makes the
exclusion hold without a blocklist: a directory a future Claude release invents
is out of the bundle the day it ships, with nobody editing anything.

Two files are genuinely both, and file-level exclusion cannot say so:

- `settings.json` carries the model, hooks and enabled plugins beside an `env`
  block whose values are environment values, which is where a token goes. The
  export keeps the names and drops the values, so the importing machine knows
  what to ask for.
- `~/.claude.json` is mostly this machine — the account, the machine id, the
  per-directory history — around the one thing worth carrying: the MCP server
  definitions. The export takes those and leaves the rest.

An import merges those two key by key rather than overwriting them. A
whole-file copy would replace the target's `env` with the bundle's blanks and so
delete the token on the machine that had one.

### What an export refuses

`apex agent profile export` walks the reusable entries and nothing else, then
re-derives the class of every file it is about to write and refuses the whole
bundle if any of them is not exportable. Nothing is half-written: the check runs
before the directory is created.

On top of that, any key that is a credential by name — token, secret, password,
api key, authorization, bearer, private key, access key — has its value emptied
wherever it appears in a file the export edits. That net is not decoration. It
was added because exporting a real profile put an HTTP MCP server's bearer token
in the bundle: it lives in `headers.Authorization`, and the rules had been
written against `env`.

Everything left behind is listed by name and class rather than dropped in
silence.

### Read-only mounts and the runtime overlay

A confined session gets the profile path by path. The reusable half is bound
read-only, so a session cannot rewrite the instructions the next one will be
started with. Session and plugin state — transcripts, shell snapshots, todos,
the plugin cache — is bound writable, because Claude writes all of it while it
runs and a read-only profile is an agent that starts and then fails in a way
that looks like a bug in Claude.

The profile directory itself is bound by nothing. `$HOME` is a tmpfs and
`bwrap` creates its own mount points, so `~/.claude` exists inside the session
as an empty writable directory with the listed entries mounted into it. That is
the runtime overlay: a file the agent invents there is writable, is private to
the session, and is gone when the session ends.

Measured with a real `claude` session under `--sandbox project`:

```
~/.claude/skills        read-only
~/.claude/commands      read-only
~/.claude/CLAUDE.md     read-only
~/.claude/settings.json read-only
~/.claude/projects      writable
~/.claude/todos         writable
~/.claude/plugins/cache writable
skills=18 home=0 ssh=absent
```

Everything that session persisted — the transcript, the session environment,
the settings backup, the rate-limit cache and `~/.claude.json` — landed on a
path the table names writable. Nothing landed outside it.

Writable directories are created before the session starts. `bwrap` binds with
`-try`, and a `-try` for a path that is not there is a no-op, so a machine where
Claude has never run would write its first transcripts into the tmpfs and lose
them at exit — which reads as the agent forgetting, not as a dropped mount.

### The doctor

`apex agent profile doctor` reads and reports; it repairs nothing, so it is
usable for finding out what state you are in. It covers config, the status line,
hooks, commands, skills, subagents, plugins, marketplaces, MCP servers and
credentials, and exits non-zero when something is wrong — a skill directory with
no `SKILL.md`, a status line that is not executable, a plugin enabled from a
marketplace this machine has never heard of.

Two things it gets right that are easy to get wrong: `enabledPlugins` is an
object in Claude 2.1 and was a list of strings before it, and a reader that
knows only the list reports a clean bill of health for a machine running seven
plugins; and `extraKnownMarketplaces` is a marketplace source in its own right,
which is the one that survives a `profile sync` onto a machine that has not run
Claude yet.

Credentials are named, never read. The doctor says where they are and that the
export does not carry them.

### Only Claude, so far

`codex`, `gemini`, `kimi` and `opencode` have no profile description, and
`apex agent profile doctor codex` says so rather than reporting an empty one.
Their sandbox keeps the whole-directory behaviour: `~/.codex` goes in writable.
Guessing which half of a directory nobody has read off a real installation is a
session store would produce exactly the failure this exists to prevent.

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
`apex project layout restore` executes. It lives under `$XDG_STATE_HOME` at `0700` and
is written only by your own runtime. It is executed as an argv **vector**, never
through a shell, so nothing in a stored entry can be interpreted as a shell
metacharacter — there is no shell to interpret it.

---

## Privilege requests

An agent has no sudo, no root shell, and a sandbox that cannot reach the system
bus. When it genuinely needs a system change, it asks:

```
apex request ask install clang --reason "Required to compile the project"
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

## The secret service

§3.2: *"Normal managed agents should not receive raw long-lived secrets where
APEX can broker the operation instead."* §11 asks for that brokering to live in
a dedicated service. That service is `apex-secretd`.

```
printf %s "$TOKEN" | apex secret add github --host github.com
apex secret capabilities                 # what the service offers
apex secret grant github git.push        # per project
apex secret use github git.push origin   # run by the agent
apex secret migrate                      # move what is already in plaintext
apex secret audit
```

You rarely type the third line. A managed session finds a `git` on its PATH
that sends `push`, `fetch` and `ls-remote` here and execs `/usr/bin/git` for
everything else, so a skill keeps running `git push` and nothing was rewritten
— §12's requirement. That shim holds no credential and enforces nothing:
`/usr/bin/git` is still there and reaches the same remotes with no credential
at all, which is exactly what happens if an agent goes round it.

### Why it is a separate daemon, and why it is root

`apex-agentd` runs as you, because it launches your own programs. Anything it
can read, a managed agent with your uid can read too — an `unrestricted` session
has your whole home. The first version of this broker kept credentials in a
`0600` file under `$XDG_STATE_HOME`, which keeps out another *account* and
nothing else.

So the store moved to `/var/lib/apex-secretd`, `0700`, owned by root, and the
agent runtime lost the ability to read a credential at all. `apex-secretd` runs
as root for two requirements that nothing unprivileged satisfies together:

1. the store must be unreadable by your uid, or the credential is still in
   reach of anything running as you;
2. the git operation must run **as** you, because it works inside your own
   repository and git executes that repository's configuration — running it as
   root would hand a local root escalation to anyone who can write a
   `.git/config`.

The daemon therefore starts privileged, keeps the store to itself, and drops to
the owner's uid for every child it forks. It never gains anything.

### Why the service performs the operation

The obvious implementation is a git credential helper the sandbox can reach. It
does not work, and the reason is worth writing down: **git runs inside the
sandbox**, so whatever the helper prints is on git's stdin, inside the agent's
own namespace, readable by the agent. A credential helper hands over the
credential by construction.

So the service performs the operation instead. The agent asks for
`git.push origin`; `apex-agentd` says which session is asking and what it is
allowed; `apex-secretd` runs the push and returns git's output.

### Where a provider plugs in

An operation is named the way §13.2 names one: `provider.thing.verb` —
`git.push`, `cloudflare.worker.deploy`, `cloudflare.r2.object.read`. The first
segment routes it, so nothing needs a table mapping operations to providers.

A **provider** supplies four things and no policy: which operations it offers,
what a resource name means (`bind`, which resolves the name and says which host
the credential would reach), how a credential is presented (`perform` — a git
credential helper, a bearer header, a signed request), and how to mint a
short-lived credential if it can (`mint`, §13.4; the default is that it cannot).

The **framework** fixes everything else, in `apex-secretd`, and a provider
cannot get any of it wrong by omission: the caller's account from
`SO_PEERCRED`, that the operation exists, that its arguments are the ones the
provider declared, expiry, the project, §7's origin, the grant, **the host
pin**, reading the value once and only after all of that, scrubbing it out of
everything returned, and the audit line.

`bind` and `perform` are separate calls because the pin sits between them. If a
provider both resolved and acted, you would have to trust it to check where it
was sending your credential. Split, the framework checks that — for every
provider anyone writes from now on.

Adding a provider is a module and one `register` call in `apex-secretd`. It
needs no change to `apex-agent-core`, `apex-agentd` or the `apex` CLI: the wire
carries an operation id, a resource and an option map, and `apex secret
capabilities` prints the service's own registry rather than a list the CLI keeps
in step by hand.

### MCP servers, and the one line of JSON that undid the store

An HTTP MCP server keeps its credential in `headers.Authorization` in
`~/.claude.json`. That file is bound **writable** into a managed session,
because Claude records onboarding state in it on every run — so the bearer
token was readable by the agent, and the store's whole argument with it.

The `mcp.request` operation closes that. It declares no resource and no
options at all, which is the point: the endpoint comes entirely from the
stored record's own host, port and path, so a session has nowhere to put a
destination of its own. `apex mcp bridge <service>` is an MCP server on stdin
and stdout that an agent spawns and talks to normally; each message goes through
`apex-agentd` — which stamps the session and checks its secret dimension — to
`apex-secretd`, which attaches the credential and makes the request.

```json
"claude-memory": {"type": "stdio", "command": "apex",
                  "args": ["mcp", "bridge", "claude-memory"]}
```

The credential reaches `curl` on its **stdin**, as a configuration file: not
argv, which `/proc` makes world-readable, and not a file, which would leave it
at rest for the length of the request. The message goes in a file instead —
only one of the two can have stdin, and the message is the caller's own.

Not carried: a server-initiated notification down a stream the server holds
open. Each message is one request and one reply. And an `Mcp-Session-Id` is
remembered per account and service, so two sessions talking to one server share
that server's idea of the conversation.

### Moving what a machine already has

`apex secret migrate` reads each plaintext credential, stores it, proves the
stored copy works, and **only then** removes the original — in that order, and
idempotent, so an interrupted run leaves a machine that still has its
credentials. It reads the old broker's `$XDG_STATE_HOME/apex/agent/secrets`,
Claude's `settings.json` `env` block, and an HTTP MCP server's
`headers.Authorization`.

There is no read-back, because the protocol has no verb that returns a value.
So verification is a *use*: `git.ls-remote` for a git credential, an MCP
`initialize` for an endpoint one. Where that cannot run — no grant yet, no
network, no repository on the right host — the credential is stored and the
plaintext is **kept**, with the reason printed. Two copies is a nuisance; none
is an outage.

It will not guess. A credential-named variable whose host nobody can work out is
named and left alone: pinning it to the wrong host and then deleting the working
copy is the one failure a migration must not have. A stdio MCP server's `env`
block is named and left too — a broker can stand in front of an endpoint, not in
front of a program running on this machine.

It refuses to run inside a session, and refuses while a `claude` with the same
`HOME` is running: that process holds `~/.claude.json` in memory and writes it
back on exit, so an edit made under it would be silently reverted.

### The API cannot return a credential

This is a property of the types, not a rule in a review checklist.
`SecretValue` implements neither `Serialize` nor `Deserialize`; every reply
derives `Serialize`. A variant that carried a credential does not compile —
today, and after every variant added for §13's Cloudflare provider or §10's MCP
header helper. The store keeps the value in a file of its own so that nothing
ever hands one to serde in the first place.

There is no `read` verb, no `export` verb and no debug escape hatch. The
`--secrets export` policy dimension exists in P0-004 as a named refusal, and
this task did not build it a path: `AgentPolicy::validate` still rejects it, and
the service has no verb it could attach to.

### The agent cannot name a URL

`git.push` takes a remote **name**, and the service resolves it against the
repository's own configuration — in the same config environment the operation
then runs in, because `git remote get-url` expands `insteadOf` and resolving
with one environment while contacting with another would pin the wrong URL.
`--push` for a write, because `pushurl` can send a push somewhere the fetch URL
never mentions.

Accepting a URL would let a session ask the service to push a branch to
`https://attacker.example/` with your credential attached — and it would,
because it was told to. The resolved host is then checked against the
credential's host, so a grant for GitHub cannot push to GitLab. An `ssh://`
remote is refused with an explanation: a token is not how ssh authenticates, and
the ssh-agent socket is masked with `$XDG_RUNTIME_DIR` by design.

### Who may change what is allowed

The store is per-uid and the uid comes from `SO_PEERCRED`, which a process
cannot forge. The pid is *not* treated as an identity — pids are reused — so it
is turned into a `/proc/<pid>` dirfd the moment the connection is accepted, and
every later question is asked through that. A reused pid makes those reads fail,
and the service refuses rather than answering about a stranger.

`add`, `remove`, `grant` and `revoke` are refused for any caller inside an agent
session, recognised by its cgroup and by its `/proc` ancestry. A confined
session cannot even reach the socket: the sandbox masks `/run` and binds back
only the agent runtime's own.

### Order of checks

Peer credentials → the session's secret dimension → project → grant → remote
name → resolved host and scheme → **then** the value is read, and only into the
environment of a child process. Every step before the last can refuse, so a
refusal cannot leak the credential through an error path. Output returned to the
caller is scrubbed as well: git does not normally print credentials, but some
error messages include a `https://user:token@host/…` URL.

### The audit trail

One JSON line per event in `/var/lib/apex-secretd/audit.jsonl`, root-owned, so
the audited party cannot rewrite the audit. Each line is §11's capability record
— provider, operation, resource, project, agent session, request origin,
approval policy, constraints, audit id — plus what the service established for
itself: the endpoint the credential was sent to, and the exit code.

```
apex secret audit
```

`origin_source` says whether §7's `request_origin` was `observed` from the
connection, `inherited` from the session, or `declared` by a client — and
`unknown` where the daemon could not read the peer's placement, never
`local-terminal`, because "could not tell" and "a human is at the keyboard" are
different answers.

You see your own account's lines. `agent_session` and `request_origin` are
**attribution, not authentication**: both are forwarded by `apex-agentd`, which
runs as you, so a process with your uid can forge them. `apex-secretd` checks
their shape — the trail is one JSON object per line and somebody greps it — and
records them as claims. They label the trail; they authorise nothing.

Origin does not gate a brokered operation, and that is §7's position rather
than an omission: its table answers `allow` for "github push" from every
origin, local and remote alike. The row it denies everywhere is "read raw
brokered secret", which this build implements by having nowhere to put it.

### What this does not protect against

Written here rather than left implied, because a boundary whose limits are not
stated gets trusted for things it never did.

* **A process running as you, outside the sandbox, can use any capability you
  granted.** It talks to the socket and presents itself as an unsessioned
  caller. What it gets is the *use* of a granted capability — never the
  credential.
* **The credential is in the environment of the `git` child while it runs**, and
  that child runs as you so the operation can touch your repository. A same-uid
  process outside the sandbox can read `/proc/<pid>/environ` during those
  milliseconds. A confined session cannot: `--unshare-pid` means the service's
  children are not in the agent's `/proc` at all.
* **A repository is caller-controlled and git reads its local config.** Every
  execution path reachable from the command line is closed — `core.hooksPath`,
  `core.fsmonitor`, the credential-helper list, `http.proxy`, `http.sslVerify`
  — and `GIT_CONFIG_GLOBAL`/`GIT_CONFIG_SYSTEM` are `/dev/null`. That is a
  mitigation maintained by hand, not a proof.
* **Root compromise ends the discussion**, here as everywhere.

The two properties it does hold: a credential is not readable at rest by your
uid, and no reply the service can send contains one.

### What is not built

Two providers: `git`, with `git.push`, `git.fetch` and `git.ls-remote`, and
`mcp`, with `mcp.request`. git is the framework's reference implementation and
the one that can be exercised without an account. Cloudflare is P1-002.

`gh`-style API capabilities (read issues, open a PR) are a second vocabulary
with a second validation surface, and `gh` inside a managed session is
unauthenticated: `~/.config/gh` is not in the profile table, so a session sees
no gh configuration at all. That was true before the broker existed and is true
now. Outside a session `gh` is untouched and works from its own `hosts.yml`,
which is also what `git push` uses there, through the `gh auth git-credential`
helper your `~/.gitconfig` already names.

Scoped-credential *issuance* — asking a provider for a narrower token per task —
is §13.4: the `mint` call exists on the provider trait and no shipped provider
implements it, so the service uses the credential it was given.

`http` is accepted only for a loopback host, where the credential does not cross
a network. It exists so the credential path can be tested end to end against a
real credential-checking server without a certificate authority in the fixture.
Only you can add a service record, and the host is pinned from then on.

---

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
| `$XDG_STATE_HOME/apex/agent/privilege-audit.jsonl` | append-only privilege audit |
| `$XDG_CONFIG_HOME/apex/agent.json` | default agent, the six permission dimensions, the network allowlist, detach key |
| `/tmp/apex-agent/<id>/` | per-session scratch, and an allowlisted session's egress socket; removed with the session |

The agent's own profile is not APEX's to keep, and APEX keeps no copy of it.
`apex agent profile inspect` prints where every part of it lives.

Transcripts are a record of your work and are readable only by you.

The secret service keeps nothing here, and that is the point of it:

| path | what |
|---|---|
| `/run/apex-secretd/control.sock` | control socket, `0666`, authorised by `SO_PEERCRED` |
| `/var/lib/apex-secretd/` | the store, `0700`, owned by root |
| `/var/lib/apex-secretd/users/<uid>/<name>.secret` | one credential, `0600` |
| `/var/lib/apex-secretd/users/<uid>/<name>.json` | its metadata — never the value |
| `/var/lib/apex-secretd/users/<uid>/grants.json` | per-project capability grants |
| `/var/lib/apex-secretd/audit.jsonl` | append-only capability audit |

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
  ask a provider for a narrower one per task. The seam is there — `Provider::mint`
  — and no shipped provider implements it. The brokering itself exists, with
  `git.push`, `git.fetch` and `git.ls-remote` as its vocabulary.
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
- **Enforcement for four permission values.** Both `--system-access` grants,
  `--secrets export` and `--origin-policy remote` parse and then refuse. The
  vocabulary is here so the enforcement slots in without moving anything else;
  see *Six permission dimensions*. All four network modes are enforced.
- **A per-project network allowlist.** The list is the runtime's, one per user.
  A project that needs a destination no other project should reach has to be
  given it globally, and §36's per-project identity is where that belongs.
