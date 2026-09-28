# The Rime agent runtime

Coding agents as a first-class OS workload, without replacing them.

`claude`, `opencode`, `codex`, `gemini` and anything else you run keep working
as they do today. Rime adds what sits underneath: the terminal they run on, the
confinement they run inside, and the project state around them.

The runtime daemon, `rime-agentd`, is not enabled on its own. Rime Remote
(`rime-remoted`, on by default for every person's account since 2026-09-23)
pulls it in through `Wants=rime-agentd.service`, so on a stock install the
runtime runs for every account that Rime Remote runs for; `docs/remote.md` has
the details. To enable the runtime directly, for example on an account where
Rime Remote is off:

```
rime agent enable
```

That works for any user, root included. As root it first gives root a lingering
systemd user instance (root has none by default), then enables the service and
prints that agents started there run as root: the per-session sandbox keeps `/`
read-only and masks the home, but the working directory is writable, so a normal
user account is safer.

---

## What it is

Four pieces:

| piece | what it is | privilege |
|---|---|---|
| `rime-agentd` | per-user daemon owning PTYs, sandboxes and session state | none |
| `rime-secretd` | system daemon owning brokered credentials | root |
| `rime agent` / `rime project` | CLI client over its control socket | none |
| `a`, `aa`, `al`, `ad`, `aw`, `ap` | shell shortcuts | none |

`rime-agentd` is **unprivileged and never talks to `rimed`**. Agent
orchestration handles untrusted model output and spawns arbitrary user
programs; putting that in the privileged daemon would make the worst case a
system compromise instead of a user-session one. When a session needs a system
change, your own `rime` invocation makes the narrow request over
`org.rimeos.Rimed1`. This daemon holds no such right.

`rime-secretd` is the one privileged piece, and it is a separate daemon for
that reason. It holds credentials and nothing else, it has no verb that returns
one, and the agent runtime is one of its clients, not its owner. See
*The secret service* below.

```
claude / opencode / codex / gemini / any binary
        │  the real upstream process, unmodified, in a real PTY
        ▼
rime-agentd  ── unprivileged, per-user, systemd --user
        ├─ PTY + session lifecycle
        ├─ bubblewrap sandbox
        ├─ adapters
        ├─ projects + git worktrees
        ├─ checkpoints
        └─ capability requests ──┐
        ▲                        │  newline-delimited JSON on /run/rime-secretd
        │                        ▼
        │              rime-secretd  ── root, system service
        │                        ├─ the store, /var/lib/rime-secretd, 0700
        │                        └─ git, run as the owner with the credential
        │  newline-delimited JSON on a 0600 Unix socket
rime agent … / Rime Shell
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
rime agent run "upgrade to Qt 7" --checkpoint --worktree qt7
rime agent list --all
rime agent attach 4
rime agent pause 4 / resume 4 / kill 4
rime agent input 4 "run the tests"            # types it, leaves it unsent
rime agent input 4 "run the tests" --submit   # and presses Enter
rime agent handoff 4 --to codex               # writes the packet, starts codex on it
rime agent handoff 4 --to codex --no-start    # writes the packet and stops
rime agent logs 4
rime agent diff 4
rime agent undo 4
rime agent allow api.example.com
rime agent default opencode
rime project info / worktrees / checkpoints
```

Pick the agent `a` runs once:

```
rime agent default claude
```

`rime agent adapters` lists what is known and what is installed.

### The PTY is the point

Rime creates the terminal, then execs the ordinary agent binary inside it. The
agent sees a normal terminal, so nothing about it has to change. Because the
*daemon* owns the terminal and your shell does not, closing the window does not
kill the work. Detach with **ctrl-]** and reattach later from anywhere.

Attaching replays the session's scrollback, so you get the screen back as it
was, then live output. Several terminals can attach to one session at once.

---

## Six permission dimensions

§3.1 names six controls that must never collapse into one switch. Each is a
separate flag with a separate default:

| # | dimension | flag | values | default |
|---|---|---|---|---|
| 1 | the agent's own permission mode | `--native` | `inherit` `ask` `bypass` | `inherit` |
| 2 | Rime filesystem/process sandbox | `--sandbox` | `unrestricted` `project` `strict` | `project` |
| 3 | Rime system/root capability | `--system-access` | `none` `session` `unsafe` | `none` |
| 4 | Rime secret capability | `--secrets` | `brokered` `none` `export` | `brokered` |
| 5 | network policy | `--network` | `open` `allowlist` `brokered` `offline` | `open` |
| 6 | remote-origin policy | `--origin-policy` | `local` `remote` | `local` |

`rime agent status <id>` prints all six for a session, and `rime agent status`
with no id prints the configured defaults: six sibling keys in `agent.json`.

### The named modes are presets over the six

§4's modes are points in that space, not a seventh setting. `rime agent run`
applies the preset first and your own flags on top, so you can still reach a
combination none of the five names, such as break-glass with the broker
switched off.

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

`rime-agent-core/tests/policy_invariants.rs` turns these sentences into tests,
each asserted over the whole value set of the dimension that drives it:

- **`bypassPermissions` does not disable the Rime sandbox.** Dimension 1 is a
  flag handed to `claude`. It cannot reach the mount namespace the sandbox is
  built out of, and the test asserts that twice: once on the policy, once on the
  `bwrap` argv the policy produces.
- **Unrestricted user does not imply root.** No sandbox value moves dimension 3.
  On top of that, a managed session runs with `PR_SET_NO_NEW_PRIVS`, so `sudo`,
  `su` and `pkexec` come up unprivileged inside it and fail. `bwrap` already set
  that for confined sessions; the runtime now sets it for the unconfined ones
  too, which is §4.3's request.
- **A root grant does not imply secret export.** No system-access value moves
  dimension 4.

### Two dimensions do talk, and only downward

`strict` forces the network dimension to `offline`, because that is what
`strict` has always meant. So `--sandbox strict --network offline` and
`--sandbox project --network offline` build the same argv, and
`--sandbox strict --network open` is refused, not quietly tightened. Nothing
loosens: `unrestricted` does not imply an open network.

`--sandbox unrestricted` with any network mode but `open` is refused. The
runtime enforces each of the other three by unsharing the session's network
namespace, and an unconfined session has none to unshare, so it would run with
a network while reporting none.

### What is refused until it is built

One value parses and is then refused: `--secrets export`, because §7's table
denies raw secret reads from every origin, the local one included. A flag that
parsed and then did nothing would read as a protection in `rime agent status`
and in a script, with nothing behind it.

`--origin-policy remote` used to be the second, and is not any more. §7 allows
remote elevation only behind a security key, and P0-014 built one, so the flag
now parses, is stored and is enforced. *Elevating from a remote origin* below
says exactly what it does and does not buy.

`--unsafe-everything --sandbox project` is refused too, for a different
reason: the pair is incoherent. `bwrap` sets `PR_SET_NO_NEW_PRIVS` on the
sessions it wraps and nothing can clear it afterwards, so a confined
break-glass session would run with the flag on while the policy reported it
off.

---

## Elevating from a remote origin

§7 gives root a different answer per origin: a human at this machine gets
"local auth", and everywhere else gets "local approval required". The second
column is what `--origin-policy remote` is about. It is the owner's opt-in to a
**different authentication**, and that authentication is no weaker.

Without it (the default), the runtime refuses a non-local caller asking for a
system-access or break-glass grant outright, whatever it sends. With it, the
runtime refuses that caller unless **all** of the following hold:

1. it presents an assertion from a credential enrolled with
   `rime agent key add`;
2. the assertion is over a challenge this daemon issued (`rime agent` asks for
   one, the key signs it, the answer comes back on the same request);
3. the challenge names *this* elevation: the session, the grant kind and the
   time limit are inside the signed bytes, so a touch collected to start a
   session cannot renew a grant, and a touch for session 7 cannot answer for
   session 8;
4. the challenge has not been spent. One issue, one attempt: a refused
   assertion burns it too, so nobody can grind against a challenge;
5. the key had a PIN: the user-verified bit has to be set inside the
   signature, because both grant kinds are root.

### polkit is not asked on this path, by design

For the local column nothing changed: the password dialog still authorises a
grant, and `org.rimeos.agent.policy` is still the action it satisfies.

For the remote column the key **replaces** polkit and is not added to it. The
reason is in the policy file itself. Both actions are `allow_any: no` and
`allow_active: auth_admin`, so a remote caller cannot pass polkit at all: polkit
refuses it under `allow_any`, and even an owner who also happened to be logged
in locally would get the dialog on this machine's desktop, which the remote
human cannot reach. Asking anyway would follow every perfect touch on an
enrolled key with a check hard-coded to say no, and §7's second column would be
a setting that could never work.

A security key is a possession factor, not a password, so no remote caller is
talking its way past a local check. The polkit defaults are correct and
unchanged; what changed is that rime-agentd no longer asks polkit about a caller
polkit has already said it has no answer for.

### The audit trail says which

`SystemGrant.authenticated_by` records the polkit action id for a password and
`security-key:<label>` for a touch, so `rime agent grants` has an
`AUTHORISED BY` column and `journalctl RIME_GRANT_AUTH=...` can tell them
apart. The prefix stops a key enrolled under the label
`org.rimeos.agent.break-glass` from producing a line that reads as a password.

---

## System-access grants

Dimension 3 is the only one that is not a setting. `--system-access session`
and `--unsafe-everything` are requests for a **grant**, and §3.3 says what a
grant has to be: "explicit, scoped, time-limited, auditable, and bound to a
concrete agent session".

```bash
rime agent run --system-access session --ttl 2h
rime agent run --unsafe-everything --ttl 15m
rime agent grants                 # what has been granted, and how each ended
rime agent revoke-grant 3         # immediate, and asks for nothing
rime agent renew-grant 3 --ttl 15m
```

### The two are different on purpose

| | `--system-access session` (§4.4) | `--unsafe-everything` (§4.5) |
|---|---|---|
| `no_new_privs` | **on** | **off**: `sudo` works |
| what it grants | the privilege verbs it names stop needing a second decision | root inside the session |
| default TTL | 30m | **none**: §3.4 says explicit |
| cap | 8h | 1h |
| indicator | the mode, in the row | **red**, with a countdown |
| expiry | the grant stops applying | the session ends |

A kernel fact drives that last row. The kernel sets `PR_SET_NO_NEW_PRIVS`
once, between `fork` and `exec`, and no process can clear it, so a break-glass
session outliving its window keeps `sudo` whatever a record says. The session
may decline a `SIGTERM`, so the runtime escalates to `SIGKILL` after a
ten-second grace and writes `session-ended` only once it has watched the
process go.

A session grant is scoped by verb **name**, not by argument: it covers
`install <anything>`, where a per-project grant covers `install clang`.
`--capabilities install,update` names the verbs a session grant covers; without
it the grant covers all eight. The name-only scope is wider on purpose: a
bounded window that only pre-approved the exact operations the user had already
approved one by one would buy nothing. It is still a whitelist, so a verb added
to the vocabulary tomorrow is not covered by a grant issued today. The grant
pre-decides; it does not pre-execute. An approved request still runs through
`rime request approve`, under the approving human's own root, and nothing in
this module runs anything.

The runtime records the two authorities apart. It files a request a per-project
grant allowed as `allow_for_project`, which is what the next identical request
in that project will find. It files a request a session grant covered as
`allow_once`, carrying the grant's id, because the window it came from can be
revoked or run out before the next request. Filing it as a standing project
grant would put a permission in the audit trail that no human ever gave and
that nothing on disk backs. The id also joins this trail to the
`RIME_GRANT_ID` line journald holds for the same window.

### Where the authority lives, and why not on disk

`rime-agentd` runs as the user. A `--sandbox unrestricted` session runs as the
user. A break-glass session is unrestricted by definition. So a granted session
can rewrite every file this daemon can write, the grant record and the JSONL
audit trail included.

So a grant holds only while **the daemon process that minted it, after a
successful authentication, still has it in memory**. The store under
`$XDG_STATE_HOME` is history: `rime agent grants` reads it and the next boot
explains it, and nothing reads it back as permission. A daemon restart drops
every grant instead of adopting one, and says so.

The trail is written twice for the same reason. The JSONL is the readable copy;
each grant event also goes to the journal, which `journald` owns as root and
which no unprivileged process can alter afterwards:

```bash
journalctl --user -t rime-agentd RIME_GRANT_EVENT=issued
journalctl --user RIME_GRANT_ID=3        # the whole life of one grant
```

### Reboot is answered, not forgotten

§3.4 asks that a grant not *silently* persist across a reboot, which is a
different requirement from forgetting it. An owner who authorised fifteen
minutes of break-glass and then rebooted has no way to tell whether the window
is still open, and a machine that loses the grant answers them with silence.

So each grant carries the boot it was issued under and holds on no other boot.
On the next start the daemon says which of four things happened, with
`/proc/stat`'s `btime` separating the first two:

| the record says | what happened |
|---|---|
| `expired` | the TTL ran out, before the reboot or since |
| `ended-at-reboot` | it was still live when the machine went down |
| `ended-with-the-runtime` | `rime-agentd` restarted while it was live |
| `revoked` | a human took it back |

The daemon writes each once, to both trails, and `rime agent grants` prints
the sentence under the table.

### Who may ask, and where the password appears

Four steps, in this order, and the order is the security property:

1. **not from inside a session.** The daemon resolves the connection through
   `SO_PEERCRED` and `/proc` ancestry to the pid it recorded when it forked the
   session. This stops an agent renewing its own grant, and it is the only form
   that check can take: the agent runs as the user, so every uid, group,
   environment variable and request field says the same thing for the agent
   and the human. The connection differs, and the kernel fills that in. An
   orphan escapes the ancestry walk and lands under `user@N.service`, which
   step 2 reads as `scheduled-job`.
2. **local origin**, per §7. A `scheduled-job`, an `mcp` server and a
   `subagent` are all non-local: the property that matters is whether a human
   is present.
3. **an origin at all.** The daemon refuses one it could not establish and
   never defaults it, because the default is `local-terminal`, which is the
   column being asked for.
4. **polkit**, with the *peer* as the subject, pinned by pid and start time.

Step 4's choice of subject carries all of §4.4's "the user authenticates
outside the agent PTY". polkit sends the challenge to the authentication agent
of the *subject's* login session, and steps 1–3 have already shown that the
subject sits outside every agent sandbox. The two actions are
`org.rimeos.agent.system-access` and `org.rimeos.agent.break-glass`, both
`auth_admin`, neither `_keep`. A renewal raises a fresh prompt, because the
prompt is worth having only while there is no standing yes to inherit.

Revoking asks for nothing. Giving up privilege is free.

### Not a stored default

Dimension 3 cannot be a default in `agent.json`. §3.4 allows no "remember
forever", and a file saying `"system": "unsafe"` would make later
`rime agent run` invocations arrive already asking for break-glass. Loading
resets that one key to `none`, leaves the other five alone, and reports the
correction.

---

## What a screen lock does

§7 gives three rules for a locked screen: ordinary agents may continue, Remote
Control may continue if configured, short-lived root grants default to
revocation, and the owner may override any of it.

```bash
rime agent lock                          # what the screen is doing, and what will happen
rime agent lock --remote continue        # Remote Control keeps working while you are away
rime agent lock --agents hold            # nothing runs unattended
rime agent lock --root-grants keep       # a grant survives the lock
```

The runtime notices within a few seconds. It stops a held session with the
same `SIGSTOP` and reports it with the same `paused` flag as
`rime agent pause`, and unlocking resumes exactly the sessions the lock
stopped. A session you paused by hand stays paused.

Revoking a break-glass grant on lock ends its session, for the reason expiry
does: `PR_SET_NO_NEW_PRIVS` was cleared between `fork` and `exec` and no
process can put it back, so a session whose grant only left the authority's
map would still have root while the record said it did not.

### Where the lock state comes from

logind's `LockedHint`, on the graphical session. Rime Shell sets it from
`WlSessionLock.secure`, the state the compositor has acknowledged and not the
request to lock, so a lock that fails to engage is never reported as engaged.

Before the shell set it, the property read `no` on a session that had been
locked for an hour, which is also what it reads on one nobody has touched. One
value for two states is not a measurement, which is why the policy shipped with
no reader behind it until the shell could answer.

There are three states, not two. A machine with no graphical session has no
screen to lock, and every rule here passes it over; the runtime treats a screen
whose state it could not read as locked, and `rime agent lock` prints the
reason. An absent `LockedHint` counts as unreadable, not as `no`:
`loginctl -p <property> --value` prints nothing and exits 0 for a property it
does not know, so a logind without the property would otherwise look exactly
like an unlocked screen forever.

A session whose origin the daemon could not establish gets the stricter of the
two "may continue" rules, because it could be either.

---

## Network modes

Four, and three of them rest on the same kernel fact. `bwrap --unshare-net`
gives the session a namespace with nothing in it but loopback: no route, no
resolver, no addresses. The modes differ in what `rime-agentd` offers on the
far side of a Unix socket afterwards. `AF_UNIX` is a filesystem object, and a
network namespace does not touch it.

| mode | IP egress | what reaches the network for it | measured |
|---|---|---|---|
| `open` | everything | the session itself | `curl https://example.com` → 200 |
| `allowlist` | none | the egress proxy, for named destinations | allowed host → 200, other host → 403 |
| `brokered` | none | the capability broker, for named operations | `rime secret grants` answers, `curl` cannot resolve |
| `offline` | none | nothing | `curl` cannot resolve |

### `brokered`

`--unshare-net` plus the broker. The daemon runs the operation outside the
namespace and returns its result; the credential never enters the session.
This is how `git push` already works from a `strict` session, and it is the
mode a cloud provider's operations are meant to be used from. `Capability` in
`rime-agent-core/src/secret.rs` is the slot a provider adds to.

`--network brokered --secrets none` is refused. The broker is the session's
only way out and `--secrets none` is what shuts it, so the pair is an offline
session under another name.

### `allowlist`

`--unshare-net` plus one route back:

```text
inside the namespace                      outside it

  agent  ──HTTP CONNECT──▶  bridge  ──▶  socket  ──▶  rime-agentd  ──▶  internet
         127.0.0.1:3128    (no policy)   AF_UNIX      (decides)
```

The bridge is `rime-agentd` re-executed with `--net-bridge`, running as the
session's parent process. It has to be inside the sandbox because the loopback
an HTTP client can reach is the session's own. It carries bytes and holds no
policy, so replacing it gains an agent nothing: the far end is still the daemon.

The runtime sets `HTTPS_PROXY` and its five spellings so a client can find the
bridge, but they do not enforce anything. A session that unsets all six does
not get a direct connection; it gets `Could not resolve host`, measured.

**Destinations** live in `agent.json` as `network_allow`, managed with
`rime agent allow`. One `host` or `host:port` per entry; no port means 443 and
nothing else; `*.example.com` covers subdomains and not `example.com`. `*.com`
and `*` are refused. One unreadable entry empties the whole list and says
which, so the mode then refuses to start instead of running one line shorter
than it looks. An empty list is refused for the same reason.

`rime agent run --allow <destination>` (repeatable, protocol 9) narrows one
session to part of that list. It can only subtract: the daemon refuses a line
the list does not cover and prints the `rime agent allow` that would permit
it. The browser capsule is built on this flag, and on two more that
`rime agent run` accepts for a session's browser, `--trust-ca` and `--present`;
`docs/browser-capsule.md` covers all three.

The daemon checks the name, resolves it, checks every address that came back,
and connects to an address it checked. Handing the name back to `connect()`
would resolve it twice, and an attacker chooses the second answer. An address
on this machine or its LAN is refused unless a rule wrote that exact address
down: `localtest.me` is a public name that resolves to `127.0.0.1`, and
allowing it by name still does not reach anything, measured.

**What it does not stop.** Only proxy-aware HTTPS goes through it: there is no
resolver in the namespace, so `ssh`, raw TCP and UDP do not work at all. A
`CONNECT` tunnel is opaque, so a session allowed to reach a host may send it
anything, in any volume: this is a destination policy and does not prevent data
loss. A name on the list is only as trustworthy as its DNS. The local-address
guard covers the case that matters here, and the client's own TLS validation
covers the rest.

Both decisions are pure functions in `rime-agent-core/src/destination.rs`, so
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

The sandbox does not own the network row: `strict` is `project` with the
network dimension forced to `offline`, and a `project` session gets whatever
`--network` says. See **Network modes** above.

Measured on Rime OS 43, kernel 7.1.5, bubblewrap 0.11.0:

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

The sandbox replaces `$HOME`, `/run` and `$XDG_RUNTIME_DIR` with empty tmpfs
mounts and binds back only an explicit allowlist. `~/.ssh`, `~/.gnupg`,
`~/.aws`, browser profiles and the ssh-agent and gpg-agent sockets are
unreachable because *nothing bound them*; no list names them. A blocklist would
be a hole every time a tool invented a new credential store.

`/run` is masked for a specific reason. `--ro-bind / /` made
`/run/dbus/system_bus_socket` visible, and it is mode `0666`. `rimed` lives on
that bus, and polkit actions that ship `allow_active = yes` (passwordless for
the logged-in local user) gate its mutating methods. A confined session *is*
that user, so `SetTier`, `SetChargeThresholds`, `Fan.SetPwm` and
`GameMode.StartForPid` were all reachable from inside the sandbox. Measured, not
theorised: `SetTier` returned success from confinement.

A denylist of known sockets could not fix that. `/run` is a tmpfs on the host
and `--ro-bind / /` binds the same filesystem, so a socket created *after* the
sandbox starts appears inside it, and anything computed at spawn time is stale
by construction. The one thing bound back is the `/etc/resolv.conf` link
target, read-only, without which every session loses DNS.

The environment works the same way: cleared, then rebuilt from locale, terminal
identity, and the specific variables the chosen adapter declares. A
`GITHUB_TOKEN` or `AWS_SECRET_ACCESS_KEY` in your shell does not reach a session
that never asked for it.

The sandbox binds back writable a few directories a toolchain needs
(`~/.cargo`, `~/.npm`, the Go module cache…), and then blanks out the
credential files that live inside them (`~/.cargo/credentials.toml`,
`~/.npmrc`).

### It fails closed

If `bwrap` is missing, or the kernel has `dev.tty.legacy_tiocsti` enabled, a
confined session **does not start**. The runtime never silently downgrades it
to a weaker policy than you asked for. The error names the escape hatch:

```
rime agent run --sandbox unrestricted …
```

### Known limits

- Escaping the sandbox is outside the threat model. This confines a
  *cooperating but fallible* agent, one that follows a bad instruction or makes
  a mistake, and not a determined kernel-exploit attacker.
- `unrestricted` confines nothing, on purpose: it is the escape hatch.
- Ordinary terminal processes are never sandboxed. Policy applies to sessions
  the runtime manages and to nothing else.
- Wayland and D-Bus session sockets are masked with the rest of
  `$XDG_RUNTIME_DIR`, so a confined agent cannot open GUI applications. The
  *system* bus is masked with `/run`, so it cannot reach `rimed` either: a
  system change has to go through `rime request` (below).

---

## The agent profile

An agent installation is a profile as well as a binary. `claude` is an
executable plus a directory of instructions, skills, slash commands, plugins
and MCP definitions that decides what the executable does, which is why two
machines on the same version behave differently.

```bash
rime agent profile list
rime agent profile inspect claude
rime agent profile doctor claude
rime agent profile export claude --to ~/claude-profile
rime agent profile sync claude --from ~/claude-profile
```

### Reusable, machine-local, mixed, secret

Every part of the profile has a class, and the class decides both what an
export carries and how a confined session mounts it. One table holds both
answers, so they cannot drift apart.

| class | what it is | exported | mounted |
|---|---|---|---|
| reusable | instructions, skills, commands, subagents | whole | read-only |
| mixed | reusable and machine-local in one file | in part | read-only, except `~/.claude.json` |
| machine-local | transcripts, caches, plugin state, install paths | no | writable |
| secret | credentials | **never** | writable |

**Anything the table does not name is machine-local.** That makes the exclusion
hold without a blocklist: a directory a future Claude release invents is out of
the bundle the day it ships, with nobody editing anything.

Two files are both reusable and machine-local, and file-level exclusion cannot
express that:

- `settings.json` carries the model, hooks and enabled plugins beside an `env`
  block whose values are environment values, which is where a token goes. The
  export keeps the names and drops the values, so the importing machine knows
  what to ask for.
- `~/.claude.json` is mostly this machine (the account, the machine id, the
  per-directory history) around the one thing worth carrying: the MCP server
  definitions. The export takes those and leaves the rest.

An import merges those two key by key and does not overwrite them. A
whole-file copy would replace the target's `env` with the bundle's blanks and so
delete the token on the machine that had one.

### What an export refuses

`rime agent profile export` walks the reusable entries and nothing else, then
re-derives the class of every file it is about to write and refuses the whole
bundle if any of them is not exportable. Nothing is half-written: the check runs
before the directory is created.

On top of that, any key that is a credential by name (token, secret, password,
api key, authorization, bearer, private key, access key) has its value emptied
wherever it appears in a file the export edits. That net earns its place. It
was added because exporting a real profile put an HTTP MCP server's bearer token
in the bundle: the token lives in `headers.Authorization`, and the rules had
been written against `env`.

The export lists everything it leaves behind by name and class, and drops
nothing in silence.

### Read-only mounts and the runtime overlay

A confined session gets the profile path by path. The reusable half is bound
read-only, so a session cannot rewrite the instructions the next one will be
started with. Session and plugin state (transcripts, shell snapshots, todos,
the plugin cache) is bound writable, because Claude writes all of it while it
runs, and a read-only profile gives you an agent that starts and then fails in
a way that looks like a bug in Claude.

Nothing binds the profile directory itself. `$HOME` is a tmpfs and `bwrap`
creates its own mount points, so `~/.claude` exists inside the session as an
empty writable directory with the listed entries mounted into it. That is the
runtime overlay: a file the agent invents there is writable, is private to the
session, and is gone when the session ends.

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

Everything that session persisted (the transcript, the session environment,
the settings backup, the rate-limit cache and `~/.claude.json`) landed on a
path the table names writable. Nothing landed outside it.

The runtime creates writable directories before the session starts. `bwrap`
binds with `-try`, and a `-try` for a path that is not there is a no-op, so on
a machine where Claude has never run, the first transcripts would go into the
tmpfs and be lost at exit. That reads as the agent forgetting, not as a dropped
mount.

### The doctor

`rime agent profile doctor` reads and reports and repairs nothing, so you can
use it to find out what state you are in. It covers config, the status line,
hooks, commands, skills, subagents, plugins, marketplaces, MCP servers and
credentials, and exits non-zero when something is wrong: a skill directory with
no `SKILL.md`, a status line that is not executable, a plugin enabled from a
marketplace this machine has never heard of.

It gets two things right that are easy to get wrong. `enabledPlugins` is an
object in Claude 2.1 and was a list of strings before it, and a reader that
knows only the list reports a clean bill of health for a machine running seven
plugins. And `extraKnownMarketplaces` is a marketplace source in its own right,
the one that survives a `profile sync` onto a machine that has not run Claude
yet.

The doctor names credentials and never reads them. It says where they are and
that the export does not carry them.

### Only Claude, so far

`codex`, `gemini`, `kimi` and `opencode` have no profile description, and
`rime agent profile doctor codex` says so instead of reporting an empty one.
Their sandbox keeps the whole-directory behaviour: `~/.codex` goes in writable.
Guessing which half of a directory is a session store, without reading a real
installation, would produce exactly the failure this exists to prevent.

---

## Projects, worktrees and checkpoints

A project is a git working tree the runtime has seen. `rime project list` shows
them by recency; `rime project info` describes the current one.

### Parallel work

```
rime agent run "fix issue 217" --worktree issue-217
rime agent run "fix issue 221" --worktree issue-221
```

Each gets its own git worktree under `.rime/worktrees/` on branch
`agent/<name>`, so two agents never fight over one checkout. The runtime
ignores the directory through `.git/info/exclude` and not `.gitignore`: it is
this machine's runtime state, not something to commit and push to your
colleagues.

Re-running with the same name reattaches to the same worktree.

### What each worktree is up to

```
rime agent worktrees
rime agent worktrees --project my-repo --json
```

```
WORKTREE               BRANCH                          DIFF  CONFLICTS   TESTS       READY
my-repo                main                           dirty  -           unobserved  uncommitted changes in the worktree
issue-217              agent/issue-217             4f +81/-12  clean       passed      yes
issue-221              agent/issue-221             2f +19/-3   1 file(s)   failed      would conflict in 1 file
```

Four questions per worktree: has it got a diff, would it merge back, what
happened to the tests, is it ready to hand over. `--json` carries the same
answers with the conflicted paths and the full blocker list.

**Nothing this command does touches a worktree you are working in.** That
constraint shapes two of the four answers, and you should understand both
before you trust the column.

**Conflicts** come from `git merge-tree --write-tree`, run from the project
root against branch names. The obvious implementation, `git merge --no-commit`
in the worktree, would leave a `MERGE_HEAD` and a half-merged index in a
checkout an agent is typing into. `merge-tree` computes the same merge entirely
in the object database.

One caveat, stated exactly, because "reads only" would be false: `--write-tree`
*does* write the merged tree and its blobs into the repository's shared object
store, as unreferenced objects that `git gc` later removes. It touches no
working tree, no index, no stash and no ref. The integration suite asserts each
part: after a status call on a conflicted worktree, `MERGE_HEAD` is
absent, `git ls-files --stage` is unchanged entry for entry,
`git status --porcelain` is byte-identical, and no ref has moved.

Base is the **main working tree's current branch, read when you ask**. Nothing
records the branch a worktree was created from, so this is an observation now
and not a memory of the branch point. Move the main tree to another branch and
every answer here is against that one instead.

**Tests are observed, never run.** The runtime does not run your suite to
answer a status query: that has a build directory, a CPU cost, and for a suite
that touches a daemon or a port a real chance of breaking the session that is
mid-task. So the column is the last test run Rime *saw go past* in that tree,
through the hook stream it already receives, and its default is `unobserved`,
which records an absence and not a failure.

What each word means:

- `unobserved`: no test run has been seen in this tree. Most worktrees.
- `running`: a run started and nothing has reported its end. A run whose
  completion never arrives stays here for as long as the daemon lives, because
  "nobody told us how it ended" is not a pass.
- `passed`: a completion event arrived for that run and it was not a failure
  event. Whether the agent upstream distinguishes those for a non-zero exit is
  upstream's behaviour, and Rime cannot compel it; if the agent ever reports a
  failed suite as an ordinary completion, this says `passed`.
- `failed`: a failure event arrived. This blocks readiness.

The runtime matches a test run to the tree the **session** lives in, taken from
the session's own recorded working directory, and not from wherever the hook
process happened to run. A session cannot report a suite result against a tree
it does not live in. The observations are process memory: restart the daemon
and everything is `unobserved` again, which is the honest answer, because
nobody here saw a test run. A `passed` written to disk would outlive the commit
it referred to and be read as a fresh verdict.

**READY is local.** The field is `ready_to_propose`, and it asks nothing of
GitHub: has an upstream, in sync with it, ahead of base, clean tree, no
conflicts, no observed test failure. It answers "is this worth a human's
attention yet", not "what does the pull request say".

`--project` takes a project **slug**, the kind `rime project list` prints, and
never a path. Answering this request makes the daemon run git in the project's
root, so the set of directories it can reach is exactly the set you have
already chosen to remember.

### Undo

```
rime agent run "upgrade to Qt 7" --checkpoint
rime agent undo
```

A checkpoint captures tracked **and untracked** files as a real git tree, plus
`HEAD`, the branch, and your installed package list. Undo restores the working
tree, deletes files created since, and unwinds commits the agent made.

In detail:

- Capture runs entirely through plumbing against a temporary index, so your
  staged changes, your stash and your branch are untouched.
- Undo takes a safety checkpoint **first**, so the undo is itself undoable.
- Checkpoints live under `refs/rime/checkpoints/`, not `refs/heads/`, so they
  never show up as branches and a plain `git push` never sends them.

Two boundaries, both on purpose:

- **Ignored files are not captured.** `.gitignore` exists to name build output
  and local secrets; sweeping a 4 GB `target/` and your `.env` into a git object
  is not an undo feature.
- **Packages are recorded, not removed.** Undo reports what was installed since
  the checkpoint and prints the `rime remove` line. A privileged, system-wide
  removal because you undid a working tree is a call you make yourself.

### A session you throw away

```
rime agent run "see if this PR is worth reviewing" --disposable
rime agent run "build it and keep the artefacts" --disposable --copy-out ~/out
```

The session runs inside a disposable capsule, and the engine deletes that
capsule at the end of the session. Rime **copies** your working directory in
and does not share it, so what the agent does to that copy goes with the
environment. Your own tree stays byte-identical afterwards, index included.

Nothing comes back unless you say where. `--copy-out DIR` copies the capsule's
`~/out` to `DIR` as the environment closes, copies nothing else, and runs
before the teardown. Leave it out and the agent's work goes with the capsule,
which is the point.

`rime agent status` names the capsule and says both of those things. A session
whose edits are about to vanish should not read like an ordinary one.

The capsule engine performs the teardown, and the daemon holds no teardown code
of its own. The engine is the session's own process, and its `trap` fires when
the agent finishes, when `rime agent kill` arrives, and when the daemon goes
away. If the machine loses power mid-session, `rime disposable list` and
`rime disposable purge` clear up what is left. Each environment carries the id
of the session that owned it, so a leftover says where it came from.

**It is a throwaway environment and not a security boundary.** distrobox
mounts the host's root filesystem at `/run/host` inside each capsule, no flag
removes it, and the process runs as your own uid. A program in there can read
and write your files. `rime disposable plan` prints the whole boundary, and
[recovery.md](recovery.md) states it in full. `--sandbox` is the mechanism for
confinement: it masks `$HOME`, puts `~/.ssh` out of reach, and rebuilds the
environment from an allowlist.

Rime **refuses these pairs instead of combining them**:

- `--sandbox` with a confining policy. bwrap would wrap the container client
  and not the agent inside the capsule, so the pair would read as "confined
  and disposable" while delivering neither.
- `--worktree`. Rime would create the branch on your machine and leave it
  empty, because the agent commits to the copy and the capsule takes those
  commits with it. A linked worktree is worse: its `.git` is a file pointing
  at an absolute host path the capsule cannot reach, so the copy is not a
  working checkout at all.
- `--checkpoint`. It would snapshot a tree this session cannot change, and
  `rime agent undo` would then offer to roll back work this agent did not do.

Each refusal lands before the daemon creates anything: no environment, no
worktree, no branch.

One interaction to know: `rime agent worktrees` lists a disposable session under
the tree you started it in, which is the tree the capsule copied. That session
cannot change it.

---

## Handing work to another agent

An agent runs out of context, or out of quota, and the work has to carry on
somewhere else. `rime agent handoff` writes down what the runtime knows about
a session and starts a different agent pointed at it:

```
rime agent handoff 4 --to codex
rime agent handoff 4 --to codex --no-start   # write it, launch nothing
```

If the work is bound to a task, hand the task over and let it find the session:

```
rime task handoff installer-bug codex
rime task handoff installer-bug codex --no-start
```

That is the same packet, written by the same command: `rime task handoff`
resolves the task to the agent session running in its root and calls
`rime agent handoff`. A packet is the record of a **session**: its transcript,
the files it changed, the worktree the runtime attributes to it, the grants it
holds. None of those can be read off a task, which is a binding. So the task
form refuses instead of guessing when the task has no session running in it,
or when it has more than one. When it refuses for that second reason it names
the ids, because the next thing to type is `rime agent handoff <id>`.

The packet is a Markdown file in the project, at
`.rime/handoff/session-4-to-codex.md`. It has to live inside the project and
not under `$XDG_STATE_HOME`: the receiving session is sandboxed, and under
`--sandbox project` the rest of `$HOME` is absent, not only hidden, so a packet
in the runtime's own state directory would go to an agent that could not open
it. `.rime/` is added to `.git/info/exclude`, not to your `.gitignore`.

`stdout` is the path and nothing else, so the command composes. Everything a
person reads goes to `stderr`. If the receiving agent fails to start, the exit
status is 1 and the packet stays where it is.

### An absent field says why it is absent

The packet has a heading for each of the nine things a handoff should carry.
Three of them have no producer in this build, and the packet writes them as
absent **with the reason**:

| field | why it is empty |
| --- | --- |
| `goal` | the opening instruction is a positional argument and is not recorded apart from the rest of the command line, which is carried instead |
| `plan` | the runtime does not record one |
| `memory project slug` | this runtime has no memory system |

`test state` used to be a fourth, and is not any more: the runtime keeps a
per-worktree test record, so the packet asks the daemon for the row that owns
the outgoing session and writes down what it says. When nobody has run a suite
there, the packet reports that as the observation it is ("Rime has not observed
a test run in this worktree"), not as a field this build cannot answer. A
recorded pass names the commit it passed AT, and says **STALE** when the
worktree has moved on since, because a pass against code that is no longer
there would otherwise persuade the incoming agent to skip the one check that
would have corrected it. If the lookup itself fails, the packet says the lookup
failed and does not claim that no suite has been run.

A plausible plan reconstructed from the transcript's first heading would be a
guess wearing the label of a fact, handed to an agent with no way to check it.
That is worse than a blank, because the receiving agent would act on it. For
the same reason the packet labels the transcript as the tail it is, and not as
the "summary" the field name asks for: nothing here decided what was important.

The packet also keeps an empty field apart from an unanswerable one. No changed
files is `Nothing has changed since the checkpoint`; a runtime that could not
look says so.

### The two kinds of grant transfer in opposite directions

Read this before you assume what the new agent can do.

- **Pre-approved project grants carry over.** A grant recorded by an approved
  privilege request is stored against the *project root*, with no session and
  no expiry, and a handoff starts the incoming agent in the same project. So
  every privilege verb pre-approved here already applies to it. Nothing is
  re-requested and nobody is asked again. Withdraw one with
  `rime request revoke`.
- **System-access grants do not.** Those are bound to a concrete session
  (§3.3), and that is the session being handed off. The packet lists them so
  the next agent knows what the work needed, and says that the agent does not
  have them; getting one back costs a local password.

The packet reports the two under separate headings for that reason. One list
would have to pick one family's semantics, and either choice is a false
statement about the other.

### The transcript is not sent anywhere

Everything in the packet was already on this machine, and the packet stays on
this machine: it is a file in your project that the next agent reads. Deleting
it loses nothing, and `rime agent handoff` will write it again.

---

## Status, and the open event protocol

`rime agent list` shows each session as `working`, `waiting_for_user`,
`permission_request`, `complete` or `failed`.

The runtime infers most of that from the terminal: a bell, an OSC 9 / OSC 777
desktop notification, OSC 133 prompt markers, silence past ten seconds, and the
exit status. Nothing scrapes pixels and nothing pattern-matches an agent's
prose.

**`permission_request` is never guessed.** No reliable method recognises a
permission prompt in arbitrary terminal output, and a wrong guess is worse than
none: it would report an agent as blocked while it works, or the reverse. Only
a published event ever sets it.

Any process inside a session can publish its own state:

```
rime agent event working
rime agent event permission_request --detail "wants to push a branch"
rime agent event complete
```

The session id comes from `$RIME_AGENT_SESSION`, which the runtime sets in every
session, so a hook script needs no arguments. That is the whole protocol: an
agent with hooks can wire them straight to it, and one without still gets the
inferred states.

---

## Handing a file to a session

A screenshot, a log, a crash dump: something in front of you that the agent
already running should look at.

```
rime agent send 3 ~/Downloads/backtrace.txt
rime agent send 3 --last-screenshot
```

The runtime copies the file into the session's own scratch directory, then
types that path into the session's terminal. The sandbox already binds that
directory read-write, and the session takes it with it when it ends.

`--last-screenshot` takes no picture and opens no selection overlay. It reads
the newest file in `~/Pictures/Screenshots`, which is where Rime Shell's Print
keybind writes. Press Print, then run it.

### It does not press Enter

The path is left on the agent's input line, and you send it. That is all that
keeps a person in the loop, because a file arrives on the same channel your
keyboard uses.

### What a program reading that terminal can and cannot tell

Bytes written to a PTY arrive as keystrokes. A terminal has no field for "this
came from somewhere else", so a language model reading its own input cannot
tell an injected byte from a typed one. Four things make the difference not
matter:

* **Only a path travels on that channel, never the contents.** The file
  reaches the model through its own read tool, where its harness already treats
  the result as data and not as instruction. Handing a file over makes it as
  trusted as `cat` would, and no more.
* **The runtime composes the text, not you.** You name a source; the runtime
  builds the destination from the session's scratch path, a counter and a name
  reduced to letters, digits, dot, dash and underscore. A file called
  `x⏎/quit⏎.png` cannot put a newline on the terminal, because the bytes on the
  terminal were never yours. A name carrying a control character gets a refusal,
  not a repair.
* **Nothing is submitted.** No newline, no carriage return.
* **Every one lands somewhere the session cannot reach**: the systemd journal,
  which also holds the mirror of every system-access grant.

  ```
  journalctl --user -t rime-agentd RIME_INJECT_SESSION=3
  ```

Two costs. The bytes land wherever that terminal's foreground process is
reading, so if the agent has opened an editor or a pager the path goes into
that instead. And someone looking at a staged path can be talked into pressing
Enter: staging slows a human down and is no boundary.

One in-band signal does exist. A terminal application that has asked for
bracketed paste (`DECSET 2004`) receives pasted text wrapped in markers, which
is how a TUI tells a paste from typing. The runtime owns the session's
terminal, so it knows whether the application asked, and it sends the markers
only then. That gives the *application* a way to know. It still gives the model
none, because the application chooses whether the distinction survives into
the prompt.

### Refused to an agent

A session may not use this verb, on another session or on itself. The runtime
reads the source with its own access, outside every sandbox, so a session that
could ask for this could name `~/.ssh/id_ed25519` and have the file carried
across the boundary for it. The daemon resolves the caller from `SO_PEERCRED`
and `/proc` ancestry, the same way it resolves a privilege request's, and
refuses anything that lands on a managed session.

`rime agent status <id>` counts the files a session has taken.

---

## Project layouts

§6 asks Rime to remember the windows and terminals of a project and restore
them after a reboot.

```
rime project layout save              # capture what is open in this project
rime project layout show              # what would come back
rime project layout restore           # reopen it
rime project layout restore --dry-run # print, start nothing
rime project layout forget
```

### What is remembered

Not window handles. A Hyprland address and a niri window id are both
meaningless after a restart, so a layout naming them could be restored exactly
zero times. The layout stores how to *recreate* each window: its argv, its
working directory, and the workspace it was on.

### Which windows belong to a project

The working directory of the process tree behind each window decides, never the
title. A title is whatever the application chose to print, and matching on it
would capture an unrelated editor that happens to have the project name on a
tab.

The catch is that a terminal's own working directory is where it was
*launched*, usually `$HOME`; the shell inside it is what moved into the project.
So the resolver checks the window's process and then its descendants,
breadth-first, and takes the first directory under the project root. Breadth
first on purpose: the shell directly inside a terminal is the directory a user
thinks of as "where that window is", not whatever a nested build step last
`cd`-ed into.

The resolver skips a window with no pid. labwc reports none, because by design
it exposes no IPC and no window-management protocol beyond the standard Wayland
ones, so on labwc `save` reports that it cannot match windows to a project
instead of guessing.

### Restoring

A terminal is *not* restored with its stored argv. That argv is typically the
bare emulator name, because the terminal inherited its working directory from
whatever launched it, so replaying it opens a terminal in the wrong place, the
most useless possible outcome of "restore my project". Instead `restore` passes
the working directory explicitly, with the flag that emulator uses. The flags
all differ, and an emulator usually treats a wrong flag as a command to run, so
the window opens, fails and closes.

An application *is* restored verbatim, because its argv carries its own
arguments.

Restore is a command and not a login hook, on purpose: a session that reopens
fourteen windows nobody asked for is worse than one that reopens none.

### Switching by project

```
rime project switch            # this project
rime project switch rime-os    # by name, from anywhere
```

§6's "allow switching by project, not only by numeric workspace". It needs a
saved layout, because the layout records which workspace a project lives on. A
project does not own a workspace; it has windows that were on one.

Where a layout spans several workspaces the most populated one wins. That is a
choice (the alternative is the first one captured), and it is the one that
matches what people mean by "where the project is".

Placement onto workspaces is best-effort. A window cannot be moved before it
exists, and it does not exist until its process has mapped a surface, which is
asynchronous and unbounded. So `restore` reports the intended split and does
not hold the terminal open for seconds guessing at startup times.

### It runs stored command lines

The layout file is a list of argv vectors that `rime project layout restore`
executes. It lives under `$XDG_STATE_HOME` at `0700`, and only your own runtime
writes it. `restore` executes each entry as an argv **vector**, never through a
shell, so nothing in a stored entry can be interpreted as a shell
metacharacter: there is no shell to interpret it.

---

## Privilege requests

An agent has no sudo, no root shell, and a sandbox that cannot reach the system
bus. When it needs a system change, it asks:

```
rime request ask install clang --reason "Required to compile the project"
```

and blocks. You see it, and decide:

```
rime request pending
sudo rime request approve 3                 # allow once, and run it
sudo rime request approve 3 --for-project   # …and stop asking for this one
rime request deny 3
```

### The vocabulary is closed

`rime request verbs` lists everything askable: `install`, `remove`,
`pkg-upgrade`, `pkg-rebuild`, `pkg-rollback`, `pin`, `rollback`, `update`. Each
maps to an `rime` subcommand that already declares itself root-only.

There is **no verb for running a command**, on purpose. An `exec` variant would
be sudo with a confirmation dialog: nobody can meaningfully review an arbitrary
shell line, and approving `sh -c '…'` once is equivalent to granting permanent
root. The request type is a Rust enum, so this is a property of the type and not
of a validation function somebody can bypass.

The daemon checks package names against rpm's own rule, which excludes `/`, a
leading `-`, and every control character. The approval prompt is what you read
to decide, so a name able to embed a newline or an escape sequence could show
you an operation other than the one being requested.

### Who is asking

The daemon resolves the asking session from the connection's **peer
credentials** (`SO_PEERCRED`), then walks that pid's `/proc` parent chain until
it meets a pid the daemon itself recorded when it forked a session.

It never reads `$RIME_AGENT_SESSION`. The runtime sets that variable inside
each session, and it is fine for `rime agent event`, where the worst a lying
client achieves is a wrong status label. But anything *authorised* by a
client-supplied id is authorised by the agent itself. The walk follows ancestry
and not the process group, because a process may `setpgid` itself and cannot
choose its parent.

A session therefore cannot approve its own request, cannot deny it, and
cannot alter its own grants. `tests/test-privilege-requests.sh` asserts all
three against a real daemon, and its negative control is a session that files a
request while claiming `RIME_AGENT_SESSION=99999` and is still attributed
correctly.

### Where the privilege comes from

`rime-agentd` is unprivileged and stays that way: §2's rule is that agent
orchestration must not live inside the privileged daemon. The daemon records,
validates and remembers; it never executes. The operation runs inside
`rime request approve`, under the same root gate as `rime install` itself, so
the privilege exercised is **yours**.

That is why a grant does not yet mean unattended execution: with nobody
present there is no privilege to borrow. Closing that gap means a privileged
executor reachable from an agent's request, and that is a new root surface, so
*Not implemented* names it and nobody has quietly added it.

### The audit trail

Every filing, decision and execution appends one JSON line to
`privilege-audit.jsonl`, which is never rewritten:

```
rime request audit
```

The runtime rebuilds the recorded `argv` from the typed verb and does not store
it as a string, so a hand-edited request file cannot smuggle an extra argument
in between the approval and the execution.

---

## The secret service

§3.2: *"Normal managed agents should not receive raw long-lived secrets where
Rime can broker the operation instead."* §11 asks for that brokering to live in
a dedicated service. That service is `rime-secretd`.

```
printf %s "$TOKEN" | rime secret add github --host github.com
rime secret capabilities                 # what the service offers
rime secret grant github git.push        # per project
rime secret grant claude-memory mcp.request --everywhere
rime secret use github git.push origin   # run by the agent
rime secret migrate                      # move what is already in plaintext
rime secret audit
```

`--everywhere` is the only line there that widens a grant past the project it
was made in, and two shipped operations qualify for it. *A grant held in every
project* below says which, and why the operation that looks like the obvious
next candidate is refused.

You rarely type `rime secret use`. A managed session finds a `git` on its PATH
that sends `push`, `fetch` and `ls-remote` here and execs `/usr/bin/git` for
everything else, so a skill keeps running `git push` and nobody rewrote
anything, as §12 requires. That shim holds no credential and enforces nothing:
`/usr/bin/git` is still there and reaches the same remotes with no credential at
all, which is exactly what happens if an agent goes round it.

### Why it is a separate daemon, and why it is root

`rime-agentd` runs as you, because it launches your own programs. Anything it
can read, a managed agent with your uid can read too: an `unrestricted` session
has your whole home. The first version of this broker kept credentials in a
`0600` file under `$XDG_STATE_HOME`, which keeps out another *account* and
nothing else.

So the store moved to `/var/lib/rime-secretd`, `0700`, owned by root, and the
agent runtime lost the ability to read a credential at all. `rime-secretd` runs
as root for two requirements that nothing unprivileged satisfies together:

1. the store must be unreadable by your uid, or the credential is still in
   reach of anything running as you;
2. the git operation must run **as** you, because it works inside your own
   repository and git executes that repository's configuration. Running it as
   root would hand a local root escalation to anyone who can write a
   `.git/config`.

The daemon therefore starts privileged, keeps the store to itself, and drops to
the owner's uid for every child it forks. It never gains anything.

### Why the service performs the operation

The obvious implementation is a git credential helper the sandbox can reach. It
does not work, because **git runs inside the sandbox**: whatever the helper
prints is on git's stdin, inside the agent's own namespace, readable by the
agent. A credential helper hands over the credential by construction.

So the service performs the operation instead. The agent asks for
`git.push origin`; `rime-agentd` says which session is asking and what it is
allowed; `rime-secretd` runs the push and returns git's output.

### Where a provider plugs in

An operation is named the way §13.2 names one: `provider.thing.verb`, as in
`git.push`, `cloudflare.worker.deploy`, `cloudflare.r2.object.read`. The first
segment routes it, so nothing needs a table mapping operations to providers.

A **provider** supplies four things and no policy: which operations it offers,
what a resource name means (`bind`, which resolves the name and says which host
the credential would reach), how a credential is presented (`perform`: a git
credential helper, a bearer header, a signed request), and how to mint a
short-lived credential if it can (`mint`, §13.4; by default it cannot).

The **framework** fixes everything else, in `rime-secretd`, and a provider
cannot get any of it wrong by omission: the caller's account from
`SO_PEERCRED`, that the operation exists, that its arguments are the ones the
provider declared, expiry, the project, §7's origin, the grant, **the host
pin**, reading the value once and only after all of that, scrubbing it out of
everything returned, and the audit line.

`bind` and `perform` are separate calls because the pin sits between them. If a
provider both resolved and acted, you would have to trust it to check where it
was sending your credential. With the two split, the framework checks that, for
every provider anyone writes from now on.

Adding a provider is a module and one `register` call in `rime-secretd`. It
needs no change to `rime-agent-core`, `rime-agentd` or the `rime` CLI: the wire
carries an operation id, a resource and an option map, and `rime secret
capabilities` prints the service's own registry, not a list the CLI keeps in
step by hand.

### MCP servers, and the one line of JSON that undid the store

An HTTP MCP server keeps its credential in `headers.Authorization` in
`~/.claude.json`. The sandbox binds that file **writable** into a managed
session, because Claude records onboarding state in it on every run, so the
agent could read the bearer token, and the store's whole argument fell with it.

The `mcp.request` operation closes that. It declares no resource and no
options at all, on purpose: the endpoint comes entirely from the stored
record's own host, port and path, so a session has nowhere to put a
destination of its own. `rime mcp bridge <service>` is an MCP server on stdin
and stdout that an agent spawns and talks to normally. Each message goes through
`rime-agentd`, which stamps the session and checks its secret dimension, to
`rime-secretd`, which attaches the credential and makes the request.

```json
"claude-memory": {"type": "stdio", "command": "rime",
                  "args": ["mcp", "bridge", "claude-memory"]}
```

The credential reaches `curl` on its **stdin**, as a configuration file: not
argv, which `/proc` makes world-readable, and not a file, which would leave it
at rest for the length of the request. The message goes in a file instead,
because only one of the two can have stdin, and the message is the caller's
own.

Not carried: a server-initiated notification down a stream the server holds
open. Each message is one request and one reply. And the service remembers an
`Mcp-Session-Id` per account and service, so two sessions talking to one server
share that server's idea of the conversation.

Nothing above says which servers a machine has, and a definition can live in
four places: your own `~/.claude.json`, its per-directory block, a repository's
`.mcp.json`, and every enabled plugin's. `rime mcp list` reads all four and
answers, per server, the two questions that matter here: where the definition
is, and whether the agent can read the credential.

```
claude-memory
  transport   http, https://mem.example/mcp
  credential  a value in the definition's Authorization header, which the agent reads
  defined in  ~/.claude.json (every directory)
  fix         rime mcp connect claude-memory

1 MCP credential is readable by any agent that runs as you
```

`rime mcp connect` is that fix, one server at a time and by hand: it reads the
credential from **stdin**, stores it, proves it against the server itself, and
only then removes it from the file the agent reads. What is left behind is the
`stdio` definition above. The order is the one *Moving what a machine already
has* argues for below, for the same reason: an interrupted run leaves a machine
that still has its credential. `--dry-run` prints the plan and writes nothing.

Listing only reads and is always allowed. Connecting is not: it refuses from
inside a session, and unless it is a dry run it refuses while a `claude` with
the same `HOME` is running, because that process holds `~/.claude.json` in
memory and writes it back on exit.

### A grant held in every project

A grant is per project, which is the right default: the same operation in a
different directory is usually a different permission. `mcp.request` and
`browser.present` are the exceptions, and `rime secret grant --everywhere` is
the exceptions' key: a `*` where the project path would go.

It is safe there for one reason. Both operations spend the credential at the
endpoint pinned when it was stored, whatever directory the request comes from,
so `*` widens *where the operation may be asked for* and not *what it reaches*.
It is worth having for `mcp.request` because an MCP server is defined once and
is therefore present in every directory: without it, every new worktree is one
where the agent's memory server is unauthorised until somebody notices. For
`browser.present` a project key would be useless, because a browser capsule's
working directory is a throwaway tree (`docs/browser-capsule.md`).

**`cloudflare.account.read` does not qualify, and it is the reason this is a
declared field and not a computed one.** The first version of the gate computed
the answer: no resource argument and no parameters, therefore nothing
project-shaped to resolve, therefore the same thing everywhere.
`cloudflare.account.read` declares no resource and no parameters, so it passes
that test exactly, and its `bind` still reads the project's own `rime.toml`.
Bound, the request is `GET /accounts/{id}` for that project's account; in a
directory that binds none it is `GET /accounts`, every account the token can
see. Two projects, two different requests, one stored token, so a `*` grant
would let an agent in a project the owner never approved read that project's
account. It is refused, and `rime cf status` needs a grant in the project it is
run in.

The correction is about where the fact lives. Naming nothing is a fact about the
**declaration**; where a request ends up is a fact about the provider's
**`bind`**, which is the same split *Where a provider plugs in* describes above,
and no amount of reading the declaration recovers it. An allow-list inside the
service would be fail-closed and silent: the next provider to add an operation
of this shape gets the safe answer, and nobody is ever asked the question.

So the question is asked, of the only party who can answer it. Every operation
declares `same_everywhere`. There is no `Default` for that struct and nothing in
the tree constructs one with `..`, so **a new operation does not compile until
its author has written down which of the two it is**, and the gate reads that
field and computes nothing of its own.

Two checks then hold the answer to account. Registration refuses the outright
contradiction: an operation that takes a resource or a parameter is a different
permission per directory by construction, so claiming otherwise is not a
judgement call. Naming nothing is *necessary and not sufficient*, and that
asymmetry carries the design. Then a test binds every operation carrying the
claim in two projects, one with an `rime.toml` that binds an account and one
bare, and requires the two results to be identical. It compares the audited
`detail` sentence and not only the endpoint, because both of
`cloudflare.account.read`'s answers are on `api.cloudflare.com`: the endpoint
alone would have passed it, and two empty directories would have passed it too.
`mcp.request` and `browser.present` are the only shipped operations that carry
the claim, and a test spells that set out, so adding one is a line somebody
writes on purpose.

### One sandbox per MCP server

§10.2. Everything above is about the credential. An MCP server is also *a
program the agent starts*, and by default it starts inside the agent's own
sandbox with everything that sandbox has: the project writable, the network, the
caches, the profile. `npx -y @modelcontextprotocol/server-memory` is third-party
code fetched from a registry at first run, given the agent's whole reach, to
store notes in one file.

`rime mcp confine` rewrites the definition so the server starts inside a sandbox
of its own:

```json
"memory": {"command": "rime",
           "args": ["mcp", "run", "memory", "--",
                    "npx", "-y", "@modelcontextprotocol/server-memory"]}
```

The server's own command stays in the definition and does not move into a
policy file, so what a server runs is still visible where somebody would look
for it. `rime mcp run` is the wrapper the agent then spawns, and it builds its
argv with the same function that confines a session. A second bubblewrap
profile in this codebase would be a second thing to get wrong, and would drift
from the one that is tested.

Three dimensions, each default-deny, and what each is worth:

**Filesystem.** Masking `$HOME`, `/run` and `$XDG_RUNTIME_DIR` costs nothing
extra, because session confinement already does it. What this adds is a
*different* home, one private directory per server, so a server that writes
beside itself writes where neither the agent nor the next server can see. The
project root is not bound unless the policy asks.

**Network.** `--unshare-net`, which is the whole of the kernel enforcement.
`network = true` hands the server the *parent's* namespace, and that is a
ceiling, not a grant: a namespace cannot be un-shared upward, so a server
declared `network = true` inside an offline session still has none. An MCP
server is not a way out of a session that was confined without one.

**Secrets**, where the honest answer is narrower than the word suggests. The
wrapper runs as the agent's own account, so neither daemon can tell it apart
from the agent: **per-MCP identity at the broker does not exist**, and a
credential this server could fetch is one the agent could fetch. What *is*
enforceable is reachability: both daemons' sockets live under the masked
directories, so by default the server can open neither. `broker = true` binds
back the one socket a confined process is ever given, `rime-agentd`'s, and the
grant table decides from there. `rime-secretd`'s socket is not bound and must
not be. A session does not get it either, and an MCP server holding a door into
the secret daemon that the agent starting it lacks would be a sandbox turned
inside out.

A policy is `<name>.toml` under `$XDG_CONFIG_HOME/rime/mcp`, and then
`/etc/rime/mcp` for a default an image or an administrator ships. The user's own
wins, because the person running a server decides what it may reach. Every
field defaults closed, so a file only ever widens. An unknown key is refused
and not ignored: a policy carrying `netwrok = true` that started the server with
no network would read as a setting that had been applied. A file that does not
parse is an error and never a quiet fall back to the default, because the
default is *tighter*, so falling back would break the server and blame the
server. The directory is bound read-only into a session, for the same reason it
is worth having.

`rime mcp policy` prints what each server will get, and where that was decided:

```
memory
  network     none — its own empty namespace
  filesystem  a private home, 0 read-only and 1 writable path(s) it names
  secrets     cannot reach rime-agentd or rime-secretd at all
  decided by  ~/.config/rime/mcp/memory.toml
  started     with everything the agent session has — rime mcp confine memory
```

Read the last line first: a policy exists and the definition still does not
use it. An endpoint server has neither policy nor wrapper. There is no process
here to confine, because `rime-secretd` makes the request, and `confine`
refuses one with that explanation instead of writing a definition that cannot
work.

This confines the MCP server's own code. It is **not** a boundary against a
hostile agent, and the limit is stated below.

### Moving what a machine already has

`rime secret migrate` reads each plaintext credential, stores it, proves the
stored copy works, and **only then** removes the original. It runs in that
order and is idempotent, so an interrupted run leaves a machine that still has
its credentials. It reads the old broker's `$XDG_STATE_HOME/rime/agent/secrets`,
Claude's `settings.json` `env` block, and an HTTP MCP server's
`headers.Authorization`.

There is no read-back, because the protocol has no verb that returns a value.
So verification is a *use*: `git.ls-remote` for a git credential, an MCP
`initialize` for an endpoint one. Where that cannot run (no grant yet, no
network, no repository on the right host) the credential is stored, the
plaintext is **kept**, and the reason is printed. Two copies is a nuisance;
none is an outage.

It will not guess. A credential-named variable whose host nobody can work out is
named and left alone: pinning it to the wrong host and then deleting the working
copy is the one failure a migration must not have. A stdio MCP server's `env`
block is named and left too: a broker can stand in front of an endpoint, not in
front of a program running on this machine.

It refuses to run inside a session, and refuses while a `claude` with the same
`HOME` is running: that process holds `~/.claude.json` in memory and writes it
back on exit, so it would silently revert an edit made under it.

### The API cannot return a credential

The types enforce this; no review checklist has to. `SecretValue` implements
neither `Serialize` nor `Deserialize`, and every reply derives `Serialize`. A
variant that carried a credential does not compile, today and after every
variant added for §13's Cloudflare provider or §10's MCP header helper. The
store keeps the value in a file of its own, so nothing ever hands one to serde
in the first place.

There is no `read` verb, no `export` verb and no debug escape hatch. The
`--secrets export` policy dimension exists in P0-004 as a named refusal, and
this task did not build it a path: `AgentPolicy::validate` still rejects it, and
the service has no verb it could attach to.

### The agent cannot name a URL

`git.push` takes a remote **name**, and the service resolves it against the
repository's own configuration, in the same config environment the operation
then runs in, because `git remote get-url` expands `insteadOf` and resolving
with one environment while contacting with another would pin the wrong URL.
`--push` for a write, because `pushurl` can send a push somewhere the fetch URL
never mentions.

If it accepted a URL, a session could ask the service to push a branch to
`https://attacker.example/` with your credential attached, and the service
would do it, because it was told to. The service then checks the resolved host
against the credential's host, so a grant for GitHub cannot push to GitLab. An
`ssh://` remote is refused with an explanation: a token is not how ssh
authenticates, and the ssh-agent socket is masked with `$XDG_RUNTIME_DIR` by
design.

### Who may change what is allowed

The store is per-uid and the uid comes from `SO_PEERCRED`, which a process
cannot forge. The service does *not* treat the pid as an identity, because pids
are reused: it turns the pid into a `/proc/<pid>` dirfd the moment it accepts
the connection, and asks every later question through that. A reused pid makes
those reads fail, and the service refuses instead of answering about a
stranger.

`add`, `remove`, `grant` and `revoke` are refused for any caller inside an agent
session, which the service recognises by its cgroup and by its `/proc`
ancestry. A confined session cannot even reach the socket: the sandbox masks
`/run` and binds back only the agent runtime's own.

### Order of checks

Peer credentials → the session's secret dimension → project → grant → remote
name → resolved host and scheme → **then** the value is read, and only into the
environment of a child process. Every step before the last can refuse, so a
refusal cannot leak the credential through an error path. The service scrubs
output returned to the caller as well: git does not normally print credentials,
but some error messages include a `https://user:token@host/…` URL.

### The audit trail

One JSON line per event in `/var/lib/rime-secretd/audit.jsonl`, root-owned, so
the audited party cannot rewrite the audit. Each line is §11's capability record
(provider, operation, resource, project, agent session, request origin,
approval policy, constraints, audit id) plus what the service established for
itself: the endpoint the credential was sent to, and the exit code.

```
rime secret audit
```

`origin_source` says whether §7's `request_origin` was `observed` from the
connection, `inherited` from the session, or `declared` by a client, and
`unknown` where the daemon could not read the peer's placement, never
`local-terminal`, because "could not tell" and "a human is at the keyboard" are
different answers.

You see your own account's lines. `agent_session` and `request_origin` are
**attribution, not authentication**: `rime-agentd`, which runs as you, forwards
both, so a process with your uid can forge them. `rime-secretd` checks their
shape (the trail is one JSON object per line and somebody greps it) and records
them as claims. They label the trail; they authorise nothing.

Origin does not gate a brokered operation, and that is §7's position, not an
omission: its table answers `allow` for "github push" from every origin, local
and remote alike. The row it denies everywhere is "read raw brokered secret",
which this build implements by having nowhere to put it.

### What this does not protect against

The limits are written here, because a boundary whose limits are not stated
gets trusted for things it never did.

* **A process running as you, outside the sandbox, can use any capability you
  granted.** It talks to the socket and presents itself as an unsessioned
  caller. It gets the *use* of a granted capability, never the credential.
* **The credential is in the environment of the `git` child while it runs**, and
  that child runs as you so the operation can touch your repository. A same-uid
  process outside the sandbox can read `/proc/<pid>/environ` during those
  milliseconds. A confined session cannot: `--unshare-pid` means the service's
  children are not in the agent's `/proc` at all.
* **A repository is caller-controlled and git reads its local config.** Every
  execution path reachable from the command line is closed (`core.hooksPath`,
  `core.fsmonitor`, the credential-helper list, `http.proxy`, `http.sslVerify`),
  and `GIT_CONFIG_GLOBAL`/`GIT_CONFIG_SYSTEM` are `/dev/null`. That is a
  mitigation maintained by hand, not a proof.
* **Root compromise ends the discussion**, here as everywhere.

The two properties it does hold: your uid cannot read a credential at rest, and
no reply the service can send contains one.

### What ships, and what is not built

Nine providers are registered (`rime-secretd/src/providers/mod.rs`): `git`
(`git.push`, `git.fetch`, `git.ls-remote`), `mcp` (`mcp.request`), `browser`
(`browser.present`), `cloudflare` (P1-002), `gdrive`, `msgraph`, `oauth`, `s3`
and `webdav`. `rime secret capabilities` prints the whole vocabulary. git is
the framework's reference implementation and the one you can exercise without
an account.

A session cannot be stopped from *un*-confining one of its own MCP servers,
because `~/.claude.json` is writable inside a session: it can rewrite a
definition to drop the `rime mcp run` wrapper, and could have run the same
program directly in any case. Closing that means starting the agent with
`--strict-mcp-config` and a configuration file the daemon wrote, which changes
how sessions are launched; it is named here and not half-built. The per-server
sandbox confines the server's code, which is a different job.

`gh`-style API capabilities (read issues, open a PR) are a second vocabulary
with a second validation surface, and `gh` inside a managed session is
unauthenticated: `~/.config/gh` is not in the profile table, so a session sees
no gh configuration at all. That was true before the broker existed and is true
now. Outside a session `gh` is untouched and works from its own `hosts.yml`,
which is also what `git push` uses there, through the `gh auth git-credential`
helper your `~/.gitconfig` already names.

Scoped-credential *issuance* (asking a provider for a narrower token per task)
is §13.4, and the Cloudflare provider does it. Before an operation runs, the
broker spends the stored token on two requests (which permission groups does
this account have, and please issue a token with exactly one of them at exactly
one scope) and then performs the operation with the token that came back, which
expires in five minutes and is deleted the moment the operation returns. The
agent holds neither token.

Asking does not always work, and the trail says which of four things happened:
the credential was narrowed, there is nothing narrower to narrow it to, the
account refused to issue one, or the attempt did not run. The last three carry
on with the stored credential, because §13.4 says *prefer*. Cloudflare requires
Super Administrator on an account to create a token, so "refused" is an ordinary
answer and not an alarming one. A project that will not accept that says so in
its own `rime.toml`:

```toml
[cloudflare]
temporary_credentials = "require"   # or "prefer", the default, or "off"
```

and then an operation that cannot be given a narrow credential is refused
instead of carried out with the broad one.

§13.4 also asks for the *tool* over the API where a tool can do the job, and
that is what a managed session's `wrangler` and `terraform` are. `rime-secretd`
runs four subcommands (`wrangler deploy`, `wrangler versions upload`,
`terraform plan`, `terraform apply`) with the credential in the child's
environment and an argv Rime writes. A skill goes on typing `wrangler deploy`;
the shim on the session's own `PATH` routes it. Anything else (`wrangler
--version`, `terraform fmt`) execs the real tool unchanged and unauthenticated,
which matches the facts: the agent has no credential. There is no "run
wrangler with my arguments" capability, on purpose, because a grant to that is
a grant to everything wrangler can do.

`http` is accepted only for a loopback host, where the credential does not cross
a network. It exists so the credential path can be tested end to end against a
real credential-checking server without a certificate authority in the fixture.
Only you can add a service record, and the host is pinned from then on.

---

## Files

| path | what |
|---|---|
| `$XDG_RUNTIME_DIR/rime-agentd/control.sock` | control socket, `0600` |
| `$XDG_STATE_HOME/rime/agent/sessions/` | session records |
| `$XDG_STATE_HOME/rime/agent/logs/` | transcripts, `0600`, capped at 32 MiB |
| `$XDG_STATE_HOME/rime/agent/checkpoints/` | checkpoint metadata |
| `$XDG_STATE_HOME/rime/agent/requests/` | privilege requests, one JSON file each |
| `$XDG_STATE_HOME/rime/agent/grants.json` | per-project "allow for project" grants |
| `$XDG_STATE_HOME/rime/agent/layouts/` | saved project window layouts |
| `$XDG_STATE_HOME/rime/agent/privilege-audit.jsonl` | append-only privilege audit |
| `$XDG_CONFIG_HOME/rime/agent.json` | default agent, the six permission dimensions, the network allowlist, detach key |
| `/tmp/rime-agent-<uid>/<id>/` | per-session scratch, and an allowlisted session’s egress socket; removed with the session. The uid is in the path: with a shared root, the second account on a machine could not start a session (P2-016) |

The agent's own profile is not Rime's to keep, and Rime keeps no copy of it.
`rime agent profile inspect` prints where every part of it lives.

Transcripts are a record of your work, and only you can read them.

The secret service keeps nothing here, and that is its purpose:

| path | what |
|---|---|
| `/run/rime-secretd/control.sock` | control socket, `0666`, authorised by `SO_PEERCRED` |
| `/var/lib/rime-secretd/` | the store, `0700`, owned by root |
| `/var/lib/rime-secretd/users/<uid>/<name>.secret` | one credential, `0600` |
| `/var/lib/rime-secretd/users/<uid>/<name>.json` | its metadata, never the value |
| `/var/lib/rime-secretd/users/<uid>/grants.json` | per-project capability grants |
| `/var/lib/rime-secretd/audit.jsonl` | append-only capability audit |

---

## Remote sessions

`--host` exists on three of these verbs, and each one means something
different:

| | what it does |
| --- | --- |
| `rime agent run --host <device>` | forwards the WHOLE invocation to that device's own `rime agent run` |
| `rime agent list --host <device>` | the sessions over there, not here |
| `rime agent attach --host <device>` | a view onto a session that keeps running there |

The run form forwards and does not reimplement. The remote applies its own
sandbox policy, its own default agent and its own checkpointing, because the
agent runs there. Reconstructing those decisions locally would make two
implementations of one policy, and the local copy would be the wrong one.
`RunArgs::forward_argv` rebuilds the flags from the parsed struct and not from
`std::env::args`, so a flag clap normalised is forwarded normalised, and the
three local-only flags (`--host`, `--remote-path`, `--allow-dirty`) cannot leak
into the remote command and make it dispatch again.

`--remote-path` names the project directory on the far side when it is not the
same absolute path, and skips the same-repository check. `--allow-dirty` runs
despite uncommitted changes here; they are NOT sent, because the remote works
from its own checkout.

The id an attach takes is the REMOTE's, which is why the list form exists.
`rime task resume` passes `host: None` on purpose: a resume attaches to a
session on this machine, and continuing one elsewhere stays explicit.

Devices come from `rime host` (§20's trusted devices), which owns the ssh argv,
including the `--` before the destination and the per-argument quoting.
`tests/test-rime-dispatch.sh` covers these forms.

---

## Terminal layouts

`rime project layout open` builds a project's terminal work in tmux or zellij:
an editor beside an agent beside a terminal, or several agents side by side.

```
rime project layout templates              # dev, review, agents
rime project layout open                   # the one this project last used
rime project layout open review
rime project layout open agents --agents 3
rime project layout open --mux zellij
rime project layout open --dry-run         # print the panes, open nothing
```

| template | arrangement | panes |
|---|---|---|
| `dev` | one large pane left, the rest stacked right | editor, agent, terminal |
| `review` | the same | editor, agent, `rime agent diff` |
| `agents` | tiled | `--agents N` agent panes, up to 8 |

The editor is `$VISUAL`, then `$EDITOR`, then the first of neovim, vim, helix
or nano that is installed. A `$VISUAL` that is not installed falls through
and is not trusted, because a pane whose command does not exist opens and dies.
The multiplexer is `--mux`, then `$RIME_MUX`, then tmux, then zellij; one that
is named but not installed is refused, not quietly substituted.

### The multiplexer is a viewport, not a host

Both the daemon and a multiplexer own PTYs, so composing them has two possible
shapes. Rime picks the one where **a multiplexer pane runs `rime agent
attach`**. The reasoning is worth stating, because the other way round looks
symmetrical and is not:

- The daemon's PTY is the durable one. `rime agent attach` is only ever a
  proxy, so killing the multiplexer, closing the terminal or logging out leaves
  every agent running, and reopening the template finds them again.
- Running a multiplexer *inside* an agent session would put the multiplexer
  server inside that session's bwrap confinement: its socket, its other panes
  and every program in them held to one agent's policy. One session could then
  hold one agent.
- It would also put the durable thing inside the ephemeral one, making the
  multiplexer a single point of failure for agent state.

Two consequences follow, and they are why this composes with no special cases.
Resize already works: `rime agent attach` turns SIGWINCH into a `Resize`
control frame, so reattaching a tmux client at a different size reaches the
daemon's PTY through `TIOCSWINSZ`. The detach key is `ctrl-]`, which collides
with neither tmux's `C-b` nor zellij's `Ctrl-p`, so you can leave an agent pane
without leaving the multiplexer.

Detaching does end that pane's `rime agent attach`. The layout builds the pane
with `remain-on-exit`, so the shape does not reflow around the hole: the pane
stays, dead. In tmux, `C-b : respawn-pane -k` brings the agent back; zellij
shows its own re-run prompt in the pane. Reopening the template attaches to the
multiplexer session as it is and does not revive a dead pane, which is why the
key is worth knowing. The agent itself was never affected: it is still running
in the daemon, and `rime agent list` still shows it.

### Attach, and restore

Reopening never rebuilds a session that is already there; it attaches to it.
Each agent pane takes the next of this project's live sessions and attaches;
when they run out, the pane starts one instead. So the same command does both
halves of "attach and restore cleanly": after a reboot there are no sessions
and the template starts fresh ones, and while agents are working it puts you
back with those agents instead of starting duplicates beside them.

The session is named `rime-<project>-<digest>`. The first half is the
project's directory name, which is what a status bar shows. The second is six
hex of its path, because `~/work/api` and `~/oss/api` are two projects, and
sharing a session name would attach one to the other's panes without saying
so.

### One layout record, not two

This is the same `rime project layout` that remembers a project's desktop
windows, not a second mechanism beside it. `save` captures the windows somebody
has open; `open` records the template it used. Both live in the one record, so
`rime project layout show` reports both halves and `forget` discards both.

`/usr/libexec/rime-mux` drives tmux and zellij, the same adapter shape as
`rime-project-windows` for compositors. The multiplexer is the only
per-backend part, so the CLI carries no tmux or zellij knowledge and the tests
have one program to fake. `rime-mux kdl <arrangement> <plan>` prints the zellij
layout that would be sent; the image build hands that straight back to zellij's
own parser.

The layout passes pane commands as argv and never through a shell, for the
reason window layouts store argv vectors: nothing in a pane command can be read
as a shell metacharacter, because nothing parses it as one.

---

## Shells

The shortcuts, completion and the prompt indicator work in bash, zsh, fish and
nushell, the four the image ships. They are not one file: bash and zsh share
`agent.sh`, and fish and nushell each get their own, because neither can source
a POSIX script. A `.` of `agent.sh` in a fish config is a syntax error, not a
degraded experience.

| shell | file | installed to |
|---|---|---|
| bash, zsh | `files/desktop/shell/agent.sh` | `/usr/share/rime/shell/`, sourced from `/etc/bashrc` and `/etc/zshrc` |
| fish | `files/desktop/fish/rime-agent.fish` | `/usr/share/fish/vendor_conf.d/` |
| fish (completion) | `files/desktop/fish/completions/*.fish` | `/usr/share/fish/vendor_completions.d/` |
| nushell | `files/desktop/nushell/rime.nu` | `/usr/share/nushell/vendor/autoload/` |

Each of those directories is the shell's own, asked of the shell and not
assumed: `fish -c 'echo $__fish_vendor_confdirs'` and
`nu -c '$nu.vendor-autoload-dirs'` both name them. Nothing edits a dotfile and
nothing runs at first login. The image build asserts two things: that each file
parses, and that the shell reads the directory it went into. The shell never
reads a file one level off, and nothing tells you.

`tests/test-shell-agent.sh` runs a real `fish` and a real `nu` for every
assertion, and compares the prompt output byte-for-byte with what `agent.sh`
produces from the same session records. It skips out loud where a shell is not
installed, and refuses to report success if every section skipped.

### What differs, and why

- **The opt-out.** `RIME_NO_AGENT_ALIASES` works in bash, zsh and fish. It
  cannot work in nushell: `def`, `alias` and `extern` are parse-time
  declarations, and putting one inside an `if` defines nothing at all instead
  of defining it under a condition. nushell's own opt-out is `hide`, in
  `~/.config/nushell/config.nu`:

  ```nu
  hide a; hide aa; hide al; hide ad; hide aw; hide ap
  ```

- **The prompt is opt-in everywhere**, and fork-free everywhere, which is the
  only way a hook that runs before every command is acceptable. fish uses
  `read -z` with a file redirect and `string match` with named capture groups;
  nushell uses `open` and `from json`. Both are builtins, so neither spawns a
  process: measured at about 0.2 ms, against the 0.25 ms bash and zsh pay.

  ```fish
  # ~/.config/fish/config.fish
  function fish_prompt
      rime_agent_prompt
      # …your prompt…
  end
  ```

  ```nu
  # ~/.config/nushell/config.nu
  $env.PROMPT_COMMAND = {|| $"(rime-agent-prompt)(pwd)" }
  ```

- **nushell completion is `extern` declarations**, which are signatures for an
  external command, not wrappers. An unknown flag or an extra argument goes
  straight through to `rime`, so a signature that falls behind the CLI costs a
  completion and never refuses a command that works. A test asserts that
  property: an `extern` that rejected valid arguments would make a working
  command look unsupported, which is worse than shipping no completion.

- **nushell reads its autoload directory in the REPL only.** `nu -c '…'` and
  `nu script.nu` do not see it, so a script that wants `a` has to
  `source /usr/share/nushell/vendor/autoload/rime.nu` itself.

---

## Escape hatches

None of this is compulsory:

- Run `claude`, `opencode`, `codex` or `gemini` directly. Nothing changes.
- `RIME_NO_AGENT_ALIASES=1` drops the shortcuts and keeps completion.
- `rime agent default` picks any adapter; `--agent generic` runs any binary.
- `--sandbox unrestricted` turns confinement off.
- The daemon is opt-in on its own, but it follows Rime Remote through
  `Wants=`, so `systemctl --user disable rime-agentd` alone does not keep it
  off. Turning Rime Remote off (`docs/remote.md`) is what keeps the runtime
  off.

---

## Not implemented

The roadmap asks for these, and this build does not do them:

- **Scoped-token issuance beyond Cloudflare.** `Provider::mint` is the seam,
  and the Cloudflare provider implements it (*What ships, and what is not
  built*, above). No other shipped provider asks for a narrower token per task;
  they use the token they are given.
- **`gh`-style API capabilities** (read issues, create a PR). A second
  vocabulary with a second validation surface.
- **Unattended execution of a granted request.** "Allow for project" means the
  next identical request needs no decision; it does not yet mean the operation
  runs with nobody present. That would need a privileged executor reachable
  from an agent's request, and a new root surface needs a design of its own.
  See below.
- **Drag-and-drop into an agent** (§3's clipboard section). `rime agent send`
  hands a session a file or the newest screenshot (*Handing a file to a
  session*); nothing takes a dropped file.
- **Test status and merge conflicts per worktree in the Agent Center** (§7).
  `rime agent worktrees` reports both on the command line (*What each worktree
  is up to*); the Agent Center shows the worktree a session is on and not
  whether its tests pass.
- **Enforcement for one permission value.** `--secrets export` parses and then
  refuses; see *Six permission dimensions*. All four network modes, both
  system-access grants and both origin policies are enforced;
  `--origin-policy remote` was the other unenforced value until P0-014 and is
  now live.
- **A session grant pre-decides, it does not pre-execute.** The verbs a
  `--system-access session` grant covers arrive already decided, and
  `rime request approve` still runs them under a human's own root. There is no
  unattended root executor, and building one would be a new boundary, not a
  smaller version of this one.
- **A per-project network allowlist.** The list is the runtime's, one per user.
  A session can narrow it (`rime agent run --allow`) and never widen it, so a
  project that needs a destination no other project should reach has to be
  given it globally. §36's per-project identity is where that belongs.
