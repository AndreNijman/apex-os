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

`$HOME` and `$XDG_RUNTIME_DIR` are replaced with empty tmpfs mounts and only an
explicit allowlist is bound back. `~/.ssh`, `~/.gnupg`, `~/.aws`, browser
profiles and the ssh-agent and gpg-agent sockets are unreachable because
*nothing bound them* — not because something listed them. A blocklist would be
a hole every time a tool invented a new credential store.

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
  `$XDG_RUNTIME_DIR`, so a confined agent cannot open GUI applications.

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

## Files

| path | what |
|---|---|
| `$XDG_RUNTIME_DIR/apex-agentd/control.sock` | control socket, `0600` |
| `$XDG_STATE_HOME/apex/agent/sessions/` | session records |
| `$XDG_STATE_HOME/apex/agent/logs/` | transcripts, `0600`, capped at 32 MiB |
| `$XDG_STATE_HOME/apex/agent/checkpoints/` | checkpoint metadata |
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
- **Privilege requests.** There is no structured "the agent asks to install
  clang" flow yet; a session simply cannot install anything.
- **Terminal layouts**, tmux/zellij integration, and restoring a project's
  windows after reboot.
- **Remote sessions.** `--host` does not exist.
- **Fish and nushell** shell integration. Bash and zsh are covered.
- **Disposable environments** and capsules.
