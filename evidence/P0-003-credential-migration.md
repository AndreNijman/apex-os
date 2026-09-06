# P0-003 — migrate agent-readable GitHub and MCP credentials to the broker

Branch `task/p0-003-cred-broker` on `AndreNijman/apex-os`, ten commits
`a0fad9d..12d75a5`, based on `roadmap/v2.2` at **d31257a**.

## Where the credentials were

Read off this machine, without printing a value:

| credential | where it lived | how it reached a session |
|---|---|---|
| a GitHub PAT, `GITHUB_PERSONAL_ACCESS_TOKEN` | `~/.claude/settings.json` → `env` | Claude applies its own `env` block to every tool it runs |
| the `claude-memory` MCP bearer | `~/.claude.json` → `mcpServers.claude-memory.headers.Authorization` | that file is bound **writable** into a session |
| `gh`'s own OAuth token (`gho_…`) | `~/.config/gh/hosts.yml` | it does not — `~/.config/gh` is not in the profile table |

The sandbox's `--clearenv` never saw the first one. It does not arrive through
the environment the process starts with; Claude reads it out of a file the
session is entitled to read, after it has started.

The PAT's value is 23 bytes and matches no GitHub token shape (`ghp_`,
`github_pat_`, `gho_`, `ghs_`, `ghu_`). The `plugin:github` MCP server on this
machine fails its handshake with *"Authorization header is badly formatted"*,
which is consistent with a stale or truncated value. Nothing in this task
depended on it being live, and it was never sent anywhere.

That has a consequence worth stating before anybody runs the migration. A real
run stores this value under the name `github` *before* verifying it. If it is
dead, the store then holds a dead credential called `github` — and the git shim
tries `github` first for any http remote. In a managed session that changes
nothing (git had no credential there anyway), but once a grant exists, a push
would get a `401` from the broker instead of falling through. If it turns out
dead: `apex secret remove github`.

## The four criteria

**1. The GitHub PAT is absent from the Claude environment.**
Enforced at spawn, not left to whether a migration has been run.
`apex_agent_core::profile::settings_without_credentials`
(`apexd/apex-agent-core/src/profile.rs:1248`) returns a copy of `settings.json`
with every credential-named `env` entry removed; `install_redacted_settings`
(`apexd/apex-agentd/src/session.rs:459`) writes it into the session scratch and
`SandboxSpec::ro_at` (`apexd/apex-agent-core/src/sandbox.rs:307`) binds it over
the real path, after the read-only allowlist and before the credential masks
(`apexd/apex-agent-core/src/sandbox.rs:549`). The name goes with the value: an
empty `GITHUB_PERSONAL_ACCESS_TOKEN` reads to most tools like a token that does
not work rather than like no token at all.

The name net is `profile::credential_name`, the same one the profile export
uses, made public rather than copied — three callers with three copies of that
list would drift, and the one that drifted would be the one that mattered.

Observed, in `tests/test-agent-profile.sh` with a real daemon, a real bwrap
namespace and a fixture home: the sentinel in the fixture's `env` block is
absent from the settings document the session reads and absent from the
session's environment, while `model`, `theme` and hooks survive. Mutation
checked — with the bind removed, two assertions fail.

**Precisely what is achieved, and what is not yet true on this machine:** the
claim is *absent from a managed session's environment*, on every session, and it
is **not live here**. Andre's `apex-agentd` is running the shipped binary; his
sessions still get the PAT until the image rebuilds and the user daemon
restarts. And it is not absent from his login shell in any case — that needs
`apex secret migrate`, which has not been run.

**2. MCP bearer secrets are brokered.**
New capability `Capability::McpRequest`
(`apexd/apex-secret-core/src/capability.rs:57`) — the only one with no
arguments, so the endpoint can only be the stored record's own host, port and
path. `broker::perform_http` (`apexd/apex-secretd/src/broker.rs:431`) attaches
the credential and makes the request; `apex mcp bridge <service>`
(`apexd/apex/src/mcp.rs:101`) is a stdio MCP server the agent spawns and talks to
normally.

The credential reaches `curl` on its stdin as a configuration file — not argv,
which `/proc` makes world-readable, and not a file, which would leave it at rest
for the length of the request. The message body goes in a file in the store's
own `run/` directory, `0711` root-owned, created `O_EXCL`.

Proved in `apexd/apex-secretd/tests/end_to_end.rs:817` against a loopback MCP
server that `401`s without a header: it received `Bearer <sentinel>`, and no
reply, no output and no audit line contained the sentinel. Event-stream and
plain-JSON answers both come back as messages; an ungranted request never
reaches the provider at all.

**3. `git` and `gh` continue to work normally.**
`apex git-shim` (`apexd/apex/src/gitshim.rs`) is written into the session
scratch as a two-line `git` and put first on a confined session's `PATH`
(`apexd/apex-agentd/src/session.rs:311`). It brokers `push`, `fetch` and
`ls-remote` against a named remote and execs `/usr/bin/git` for everything else
— every flag, every refspec, every other subcommand, any remote given as a URL.
A refusal about the *remote* (`"no credential stored"`, `"is not an http
remote"`, `"but this credential is for"`, `"has no remote called"`) falls through to real
git; a refusal about a *grant* does not,
because replacing "not granted" with git's authentication failure sends the
reader to the wrong place.

It holds no credential and enforces nothing. `/usr/bin/git` is still there and
reaches the same remotes with no credential at all, which is the whole of what
happens if an agent goes round it. Every check that matters is in the daemon.

Observed in a real confined session (`tests/test-secret-broker.sh`):
`command -v git` is the shim, `git --version` is git's own answer, `git fetch
origin` returns the broker's, and git never asked for a credential.

**`gh` is where this criterion is met for `git` and unchanged-not-met for
`gh`.** Measured, not inferred. Inside a managed session it says *"You are not
logged into any GitHub hosts"* — `~/.config/gh` is not in the profile table, so a session has never
seen gh's configuration, before this change or after — so "continue to work
normally" is satisfied for `gh` only in the sense that nothing was broken; a
session still cannot use it, and brokering its API operations is P1-018.
Outside a session, `gh` reads
`~/.config/gh/hosts.yml` as it always did, and `git push` on github.com goes
through the `credential.https://github.com.helper = !/usr/bin/gh
auth git-credential` line already in `~/.gitconfig`. Nothing under
`~/.config/gh` was read for a value or written.

**4. Secret values are not visible inside managed Claude.**
Three separate reasons, none of which is a policy check:

* the store is root-owned `0700` (P0-002) and the secret service's socket is
  not bound into the sandbox at all;
* `SecretValue` implements neither `Serialize` nor `Deserialize`, so no reply
  can carry one — unchanged, and the two new wire fields are a `body_len` and a
  message, never a value;
* the two files that did carry one are now a redacted copy (`settings.json`)
  and a definition naming the bridge (`.claude.json`).

## The migration, and what a real run would do

`apex secret migrate` (`apexd/apex/src/migrate.rs`). Per credential: **store,
verify, and only then remove**, every step idempotent. An interrupted run leaves
plaintext a later run reads again, which is a machine that still works.

Verification cannot be a read-back — the protocol has no verb that returns a
value, which is the point — so it is a *use*: `git-ls-remote` for a git
credential, an MCP `initialize` for an endpoint one. Where that cannot run the
credential is stored and the plaintext is **kept**, with the reason printed.

Guards: it refuses inside an agent session, and refuses while a `claude` with
the same `HOME` is running — that process holds `~/.claude.json` in memory and
writes it back on exit, so an edit made under it would be reverted and the
credential would be back in the file with nothing to say it had left. "The same
HOME" is checked through `/proc/<pid>/environ` rather than assumed, so another
account's Claude does not block it.

**Proved on a fixture** (`tests/test-secret-migrate.sh`, 34 assertions): a
fixture home under `/var/tmp` with two obvious fakes, a private secret service,
and a loopback MCP server that `401`s without the header. The first run stores
both and removes neither. After one grant, the second run sends
`Bearer <fake>` to the server, removes the token from `~/.claude.json`, and
rewrites the definition to `apex mcp bridge fixture-memory` with the rest of the
document and the `0600` mode intact. A third run finds nothing. `ACME_API_KEY`
— a credential whose host nobody can work out — is named and left, and so is a
stdio server's `env` block.

The old broker's leftovers go all the way through in **one** pass, because
their grants come with them: stored, grants carried, verified by a real
`git ls-remote` against a loopback smart-HTTP server that `401`s without a
`Basic` header, and only then is the `0600` JSON file deleted. A keyring-backed
record — metadata present, value absent — is named and **not** deleted, because
there is nothing to carry and the record is the only trace of what was there.
The grants have to follow the credential and not precede it: the service
refuses a grant for a credential that is not stored, which is the right refusal
and was an ordering bug this test found.

**Andre's real credentials were not migrated.** A real run on this machine
would, with Claude closed:

1. store the PAT as `github`/`github.com`, try `git-ls-remote origin` in
   whatever repository it is run from, and — with no grant yet, and with a
   value that matches no GitHub token shape — fail that check and **leave the
   plaintext in `settings.json`**, printing why. That is the discipline working:
   an unverifiable credential is not deleted.
2. store the `claude-memory` bearer as `claude-memory`,
   `https://memory-vps.andrenijman.com/mcp`. With `apex secret grant
   claude-memory mcp-request` given first, the `initialize` handshake would
   verify it, the header would be removed from `~/.claude.json`, and the server
   would become `apex mcp bridge claude-memory`. Without the grant it is stored
   and left.
3. find nothing in `$XDG_STATE_HOME/apex/agent/secrets/` — that directory is
   empty on this machine, so P0-002's "leftover plaintext" is a real path with
   nothing in it here. On a machine that *does* have leftovers the path is
   exercised end to end in the fixture below.

## A dry run against this machine

Run from `/var/home/andre/Projects/apex` with `--dry-run`, which writes nothing
and is deliberately allowed past the running-agent guard so somebody can see
what a real run would do without closing Claude first:

```
would  store github (https://github.com) from ~/.claude/settings.json → env → GITHUB_PERSONAL_ACCESS_TOKEN
would  store claude-memory (https://memory-vps.andrenijman.com/mcp) from ~/.claude.json → mcpServers → claude-memory → headers

nothing was written. Run without --dry-run to migrate.
```

Two credentials, correctly identified, no value printed, nothing changed. A run
without `--dry-run` refuses on this machine right now: `claude` is running as
Andre in this home, and the process names are `claude` *and* `claude.exe`, which
`pgrep -x claude` finds only half of — so the guard matches `argv[0]`'s basename
instead.

## Verified

On katana (20 cores), branch head:

* `cargo test --manifest-path apexd/Cargo.toml` — **1466 passing, 0 failing**
  (baseline 1436).
* `cargo clippy --all-targets --locked -- -D warnings` in the pinned clippy
  image — clean.
* `tests/test-secret-broker.sh` — 61 passing (was 57).
* `tests/test-secret-migrate.sh` — 34 passing, new, wired into pr-validation.
* `tests/test-agent-profile.sh` — 48 passing (was 39).

## What this closes of P0-002, and what it does not

P0-002 shipped `partial` for two named reasons.

**Closed.** *"An upgraded machine still has the old broker's plaintext files
with nothing to migrate them."* `apex secret migrate` reads
`$XDG_STATE_HOME/apex/agent/secrets/*.json` in the format at `8af6ccc^`, stores
each one, carries its per-project grants from `secret-grants.json`, verifies,
and removes the file — proved in the fixture above, including that a
keyring-backed record with no value is named rather than deleted. P0-002's
`apex secret list` warning about that directory now names a command that exists.

**Still open, untouched.** *"The daemon has never actually run as root in
testing, so the at-rest half of criterion 1 is argued from the unit file rather
than measured."* Every test here still starts `apex-secretd` with `--store` and
`--socket` as an ordinary user, where the setuid drop is a no-op and `hello`
reports `protected: false`. This branch makes that gap slightly more
load-bearing rather than less: `perform_http`'s `O_EXCL` and its `0711`
directory exist for the case where the daemon *is* root, and that case has not
been exercised. It needs an image build and a boot.

So: **P0-002 stays `partial`**, with one of its two reasons gone and the other
unchanged.

## Not done, and named

* **A per-project grant for an MCP server is friction.** An MCP server is
  global and a grant is per project, so `claude-memory` is unauthorised in every
  new worktree until somebody grants it there. A `"*"` project key would fix it
  and is a policy widening, so it is not done silently.
* **`gh` API operations are not brokered.** A second vocabulary with a second
  validation surface; P1-018.
* **A server-initiated MCP notification** — down a stream the server holds open
  — does not arrive. Each message is one request and one reply.
* **One `Mcp-Session-Id` per account and service**, so two sessions talking to
  one server share that server's conversation.
* **`--settings '{"disableAllHooks":true}'` on the agent's own command line**
  still silences the hook bridge (P0-011's note). It does not affect any of
  this: the settings bind and the PATH entry are fixed at spawn.
* **The daemon has still never run as root in testing** (P0-002's gap,
  unchanged). The at-rest half of the store boundary is argued from the unit
  file. `perform_http`'s `O_EXCL` and `0711` directory matter most when it *is*
  root, and that path has not been exercised as root.
