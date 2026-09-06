# The protected secret service

`apex-secretd` holds the credentials a managed agent may *use* and must never
*hold*. Roadmap §11 and §12; the task is P0-002.

The one sentence it exists for: **an agent asks for an operation, the service
performs it, and the result comes back without the credential.** Everything
below follows from that.

## Why a third daemon

APEX already has two daemons near this problem and neither can do the job.

`apex-agentd` runs as the user, because it launches the user's own programs and
handles untrusted model output. That is right for what it does and fatal for a
credential store: a managed agent runs under the same uid, so anything
`apex-agentd` can read, the agent it started can read. A `0600` file under
`$XDG_STATE_HOME` is protected from *other accounts*, and other accounts are not
the threat.

`apexd` is root, owns system policy, and has a frozen D-Bus surface with polkit
actions behind it. Credentials there would put every credential operation across
a privileged interface, and a polkit prompt in the path of an agent nobody is
watching.

So: a third daemon, under its own system account, owning one directory no human
user can open. It is not privileged. It holds no capabilities, touches no device
but the TPM, and can do exactly one thing no other process can, which is read
the store.

## Where things live

```text
/var/lib/apex-secretd/                 0700  apex-secret
  owners/<uid>/secrets/<name>.json     0600  metadata, never a value
  owners/<uid>/secrets/<name>.blob     0600  the value, sealed or plain
  owners/<uid>/grants.json             0600
  owners/<uid>/audit.jsonl             0600
/run/apex-secretd/broker.sock          0666  metadata, and performing operations
/run/apex-secretd/admin.sock           0600  store, rotate, grant, revoke
```

Every directory on the way down is `0700`, not only the leaf. A traversable
parent leaks the names of the credentials inside it, and the names say which
providers a user holds credentials for.

One directory per owning uid, rather than one file with a uid column. The uid is
a path component the caller never supplies, so a bug in name handling cannot
reach across accounts even if validation were bypassed.

## The two sockets

The broker socket is open to every local uid, because every uid has its own
namespace behind it. Which namespace is decided by `SO_PEERCRED`, which the
kernel fills in at `connect(2)` and no client can choose. Connecting as somebody
else is not possible; connecting as yourself reaches your own store and nobody
else's.

The admin socket is `0600` and owned by `apex-secret`, so in practice only root
can open it. That boundary is what makes a grant mean something. A managed agent
runs under the owner's uid; if the owner could grant a capability over the
broker socket, so could the agent, and a permission an agent can give itself is
not a permission. The cost is that storing and granting need `sudo` — the same
shape as `sudo apex ai pull` writing the shared model store.

## What the API can return

Not a credential. Three overlapping mechanisms, because one of them can be
forgotten:

1. **The type wall.** `SecretValue` implements neither `Serialize` nor `Display`
   nor `Deref` nor `AsRef<str>`. `Response` derives `Serialize`. A response
   variant carrying a value does not compile. The single accessor is
   `expose()`, and a test greps the crate's shipped source to assert it is
   called from exactly two modules: the one that attaches a credential to a
   request, and the one that seals it.
2. **The classification wall.** `payload_kind` matches exhaustively over every
   response variant and classifies it as metadata, an operation result, an
   acknowledgement or an error. There is no variant of `Payload` meaning "a
   credential". Adding a response fails to compile until somebody classifies it,
   and none of the four options can hold a value.
3. **The wire sweep.** `apexd/apex-secretd/tests/wire.rs` runs the real binary,
   stores a sentinel, drives *every* verb the protocol has — success paths and
   failure paths — and asserts the sentinel appears in none of: the raw bytes of
   any reply, the audit log, or the daemon's own stderr. The sweep checks its
   own coverage against the verb list, so a verb added later cannot slip past
   it.

Values do travel *in*: `store` and `rotate` carry one. That is `InboundSecret`,
which redacts itself in `Debug`, so a daemon that logs a request it could not
handle cannot log a credential.

## The operation vocabulary

An operation is a named, closed thing — `github:whoami` — not "issue this
request with my token attached". A vocabulary a caller can extend is one in
which "use the credential against a host I control" is a word, and no reviewer
can meaningfully approve a grant written in it.

A caller supplies two pieces of free text, both looked up in tables: the
credential's name and an operation id. Plus an optional `resource`, validated
against that operation's own shape — `owner/repo`, two segments of
`[A-Za-z0-9._-]`, so it cannot walk out of the path it is substituted into. The
method, the endpoint, the headers and the fields that may come back are
constants.

What comes back is the HTTP status and an allow-list of named scalar fields.
Never the body: an API that echoes a request header into an error message would
otherwise hand the credential straight back. The extraction refuses objects and
arrays, and the result is scrubbed of the value on top of that.

Requests are made with `curl --config -`, the configuration on stdin. The
credential is therefore never in `argv` (`/proc/<pid>/cmdline` is
world-readable), never in an environment variable, and never in a file left
behind by a crash. Redirects are off: a 302 is the provider choosing a new host
for a request that carries a credential.

## The capability record

§11 names ten fields, and all ten are recorded. They are not worth the same.

| field | established by |
|---|---|
| `provider`, `operation`, `resource` | the closed vocabulary |
| `owner_uid`, `peer_uid`, `peer_pid` | the kernel, via `SO_PEERCRED` |
| `expiry`, `constraints`, `approval_policy`, `audit_id` | the service |
| `project`, `agent_session`, `request_origin` | **the caller, unverified** |

The last row is wrapped in a `Claimed<T>` type. `apex-secretd` runs under its
own uid: it cannot read another user's `/proc/<pid>/cwd` to check a project, and
it has no session table — `apex-agentd` has that, because it forked the sandbox.
So those three are recorded and never consulted by policy. An audit trail that
does not say which of its fields the subject chose reads as more than it is.

## What this does not protect against

Stated plainly, because a design note that overstates is worse than none.

- **Root.** Root can read `/var/lib/apex-secretd` directly. No API returns a
  value to anyone, root included, and a root grant therefore comes with no
  export path — but a root *shell* does not need one. The `security_invariants`
  line "root/system grants must not implicitly export raw brokered secrets" is
  about the API surface, and that is where it is kept.
- **Sealing does not change that.** `--seal` encrypts to this machine's TPM, so
  a copy of the disk taken elsewhere is inert. Locally the TPM will unseal for
  anything that can reach `/dev/tpmrm0`, and root can. Binding to PCRs would
  narrow it to one boot state at the cost of breaking every sealed value on a
  firmware update; that needs a rewrap path first.
- **Sealing is off, and on a stock host unavailable.** Measured on Fedora 43:
  `systemd-creds encrypt --with-key=tpm2` succeeds as root and fails for a
  non-root account with `io.systemd.InteractiveAuthenticationRequired`. This
  service must never raise a prompt nobody is watching, so it reports the
  refusal instead of storing the value unsealed. `Plain` is the default and the
  uid boundary is what protects it.
- **The pid is not an identity.** `SO_PEERCRED` gives a pid that is true at
  `connect(2)` and decays immediately: the peer may exit and its pid be reused
  before anything reads `/proc`. The connection therefore pins the peer's start
  time and records `0` once it no longer matches, rather than naming a stranger.
  This makes the audit accurate; it is not a privilege boundary, because
  authorisation never depended on the pid. The window between `connect(2)` and
  the first `/proc` read is unclosable from userspace — `SO_PEERPIDFD` closes
  it, and needs a `libc` version this workspace does not yet pin.
- **A confined agent reaching the socket at all.** The per-session sandbox does
  not currently bind-mount `/run/apex-secretd`, so a `project`-policy agent
  cannot reach the service directly. Relaying through `apex-agentd`, which knows
  the session it forked and can narrow by project before passing a request on,
  is the intended shape and is not built here.
- **A credential in memory.** `SecretValue` overwrites its buffer on drop. That
  clears the allocation; it does not reach a copy a `String` made during a
  reallocation, a page the kernel swapped out, or a core dump taken while the
  value was live.

## Using it

```bash
# What can be brokered.
apex capability providers

# Put a credential in. sudo, and the value on stdin — never on the command line.
printf %s "$TOKEN" | sudo apex capability store github --provider github

# Nothing is allowed until it is granted. Also sudo.
sudo apex capability grant github whoami
sudo apex capability grant github repo-metadata --resource AndreNijman/apex-os

# What an agent does. No privilege needed.
apex capability use github whoami
apex capability list
apex capability grants
apex capability audit
```

`apex secret` is a different, older thing: the per-user broker inside
`apex-agentd`, whose store is a `0600` file in `$HOME` and which performs `git`
operations across a namespace boundary rather than a uid one. It still works and
is untouched. Moving what it holds into this service is separate work.

## What is not here yet

- **Filesystem-bound operations.** `git push` needs both the credential and read
  access to the user's repository. This service has the first and, running under
  its own uid with `ProtectHome=yes`, deliberately not the second. Three ways
  out, none chosen here: run the unit as root and drop to the caller's uid per
  operation, losing most of the hardening; add a narrow privileged helper, which
  AGENTS.md's culture is against; or take §3.2's other branch and mint a scoped,
  short-lived credential for `apex-agentd` to use *outside* the sandbox. The
  record models `expiry` for the third; no API returns a credential, so none of
  it is reachable today.
- **A policy layer worth the name.** `PolicyInput` carries `origin` and
  `root_peer` and both are inert. §7 wants remote-origin requests
  distinguishable from local ones, and a local socket peer is local by
  construction — "this came from Remote Control" is a statement only an
  authenticated relay can make, and there is not one. Splitting those dimensions
  is P0-004's work; this is the struct it can fill in.
- **Per-use approval.** `ApprovalPolicy::PerUse` exists in the record and is not
  implemented. Nothing here can ask the owner a question, and a broker that
  blocked on an unanswered dialog would hang every agent that called it.
