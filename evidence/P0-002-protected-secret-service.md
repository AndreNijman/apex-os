# P0-002 — protected `apex-secretd` service

Branch `task/p0-002-secretd-2` on `AndreNijman/apex-os`, head **8f7322d**,
rebased onto `roadmap/v2.2` at **233d2d9** (P0-008, P0-013/014/015, the EACCES
sweep, the default firewall and the SBOM step).

## What was built

Two new crates in the `apexd/` workspace:

| crate | what |
|---|---|
| `apex-secret-core` | the model: capability record, wire protocol, store, audit, client |
| `apex-secretd` | the daemon: peer identification, the verbs, the git broker |

`apex-agentd` kept the session knowledge and lost the credentials.
`apex-agent-core/src/secret.rs` was deleted rather than emptied: there is no
store, no `token()` and no keyring backend left in the agent runtime.

## The five acceptance criteria

**1. Secrets are no longer stored in agent-readable home paths.**
The store is `/var/lib/apex-secretd/users/<uid>/`, `0700`, owned by root, with
the value in a file of its own (`<name>.secret`, `0600`) separate from its
metadata. The unit sets `StateDirectory=apex-secretd` and
`StateDirectoryMode=0700`, and `Containerfile.base` asserts both, because a mode
that drifted to `0755` is silent at runtime.

**2. The normal API cannot return raw secret values.**
`SecretValue` implements neither `Serialize` nor `Deserialize`; `Response`
derives `Serialize`. A reply carrying a credential does not compile. The store
keeps the value out of serde entirely by putting it in its own file, and
`Request::Add` carries a length with the raw bytes following the request line,
so the inbound direction has no exception to explain either.

Three tests keep it true as the API grows:
`protocol_surface_is_pinned` (an exhaustive match plus a pinned name list, so a
new variant fails to compile and then fails a test),
`no_reply_carries_a_credential`, and `nothing_the_socket_can_answer_contains_the_credential`
against a live daemon holding a sentinel.

**3. The broker can execute credential-backed operations.**
`git-ls-remote` was chosen for the end-to-end proof, with `git-fetch` asserted
down the same path. `apexd/apex-secretd/tests/end_to_end.rs` starts the real
binary on a real socket, points a real git repository at a git smart-HTTP server
on loopback that answers `401` without an `Authorization` header, and asserts
that the server received `Basic base64(user:sentinel)` while no reply, no output
and no audit line contained the sentinel.

**4. Audit entries exist for secret use.**
`/var/lib/apex-secretd/audit.jsonl`, root-owned, one JSON object per line. Each
line is §11's record — provider, operation, resource, project, agent_session,
request_origin, approval_policy, constraints, audit_id — plus the endpoint the
daemon resolved and the exit code. The trail lives with the store, so the
audited party cannot rewrite the audit; in the old broker it was a user-writable
file next to the session transcripts.

**5. The service is separate from `apex-agentd` and from broad `apexd`.**
A new crate, a new binary, a new system unit, its own socket and its own
protocol. `apex-agentd` is one of its clients.

## What the rebase had to preserve

Three tasks ended up in `apex-agentd/src/broker.rs` and all three survive.

* **P0-013's provenance.** Deleting `apex-agent-core/src/secret.rs` would have
  taken `AuditEntry`'s `origin` and `origin_source` with it. Both moved into
  `CapabilityRecord` and `AuditLine`, so a secret-use record still says where a
  request came from *and* whether the daemon observed that, inherited it from
  the session, or was asked for it. An origin the daemon could not read is
  `unknown`, never the `local-terminal` default. The shell suite asserts no
  line claims a local origin it did not observe.
* **P0-008's brokered network mode.** The module note explaining why an
  `AF_UNIX` socket is how a session with `--unshare-net` reaches a provider at
  all is kept, updated for the fact that two daemons are now outside the
  namespace instead of one.
* **`may_use_broker` still takes no origin.** §7's table answers `allow` for
  "github push" from every origin, so origin is recorded and does not gate.
  Adding a parameter would have been an invitation to tighten a row the roadmap
  says is open.

`PROTOCOL_VERSION` went to **4** with a named guard,
`BROKERED_SECRET_SERVICE_VERSION`. `SecretGrant` and `SecretGrants` left the
agent protocol — a grant is now a change to `apex-secretd`'s own store — and
`Brokered` gained `audit_id` and `endpoint`. The failure the guard prevents:
`apex secret use` against a daemon below 4 would be served by that daemon's own
broker reading its own old store, and the user would be told they never stored
a credential they had just stored.

## What is verified, and what is not

Verified on this machine:

* `cargo test --workspace` — **1351 passing, 0 failing** (baseline on 233d2d9 is
  1273; the agent runtime's ~30 secret tests were deleted with the module and
  ~110 new ones added).
* `cargo clippy --all-targets --locked -- -D warnings` in
  `docker.io/library/rust:1-slim` — clean.
* `tests/test-secret-broker.sh` — **57 passing, 0 failing**, including a real
  confined session that cannot read the store, cannot reach the secret service
  at all, and cannot grant itself a capability.

**Not verified**: the daemon has never run as root. Every test starts it with
`--store` and `--socket` as an ordinary user, where the setuid drop is a no-op
and `hello` reports `protected: false`. So the *at-rest* half of criterion 1 —
that a process with the user's uid cannot open the store — is argued from the
unit file and the mode bits rather than measured. It needs a build and a boot.

**Left behind, and named rather than deleted**: an already-installed machine
still has the old broker's credential files in
`$XDG_STATE_HOME/apex/agent/secrets/`, plain JSON that anything running as the
user can read. Nothing reads them any more and nothing removes them — a file
that may hold the only copy of a token is not something a `list` command should
delete on its own — so `apex secret list` names the directory and says what to
do. Until somebody does, criterion 1 is true of new credentials and not of old
ones on that machine.

A real migration is worth doing and is not this task: reading each old record,
handing the value to `apex-secretd`, carrying the per-project grants across, and
shredding what is left. It belongs with P0-003, which is already the task that
moves live credentials, or with P1-029's persistent-state migration framework —
one mechanism for this rather than a bespoke one.

**Not done, deliberately**: the real GitHub and MCP credentials are not
migrated. That is P0-003.

`SecretPolicy::Export` is still refused by `AgentPolicy::validate`, and the
service has no verb it could attach to. P0-004's note called it "a loose end for
P0-002's owner-controlled path"; this task chose not to build that path, because
a path that returns a value is the thing criterion 2 forbids.

## What the threat model does not cover

Written into `apex-secret-core`'s crate note and into `docs/agent-runtime.md`,
not only here:

* a process running as the user, outside the sandbox, can *use* any capability
  the owner granted — session identity comes from `apex-agentd`, which runs as
  the user, so it is attribution and not authentication. It never gets the
  credential;
* the credential is in the environment of the `git` child while it runs, and
  that child runs as the user so the operation can touch the user's repository.
  A same-uid process outside the sandbox can read `/proc/<pid>/environ` during
  those milliseconds; a confined session cannot, because `--unshare-pid` keeps
  the daemon's children out of the agent's `/proc`;
* a repository is caller-controlled and git reads its local config. Every
  execution path reachable from the command line is closed by `-c` flags and
  `GIT_CONFIG_GLOBAL`/`GIT_CONFIG_SYSTEM=/dev/null`. That list is maintained by
  hand;
* the "not from inside a session" check on the mutating verbs rests on cgroup
  membership and `/proc` ancestry, and a determined same-uid process can move
  its own cgroup or double-fork. It is hardening, not the confidentiality
  boundary;
* the socket is `0666` with a thread per connection and no cap, so any local
  account can exhaust the daemon's threads. `apex-agentd` has the same shape.
  That is a denial of service, not a disclosure.

The two properties that do hold: a credential is not readable at rest by the
owner's uid, and no reply the service can send contains one.
