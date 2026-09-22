# APEX Remote — `apex remote`

Pairing a phone with this machine, and taking it away again.

The phone talks to `apex-remoted`, a **per-user, unprivileged** service that is
a *client* of the agent runtime rather than a part of it. Everything it
forwards is recorded as `claude-remote-control`, so a remote request can edit a
project, run tests and push — and cannot approve a root operation or start a
break-glass session, whichever device asks. That boundary is the point of the
feature: the answer to "what can somebody do with my phone" is bounded by what
the origin `claude-remote-control` may do, not by who is holding it.

Reading and revoking need no privilege. **Pairing needs you**: `apex remote
pair` is refused by `apex-remoted` for any caller that does not classify as a
§7 local origin, which an agent inside a managed session never does. An agent
cannot pair a device on your behalf.

Five verbs.

## `apex remote enable`

Turns APEX Remote on for this account — `systemctl --user enable --now
apex-remoted`, the same shape as `apex agent enable`, because a per-user daemon
that has to be started with a `systemctl` invocation is one people get wrong.

It does **not** open the firewall, and says so. LAN access needs

```
sudo apex firewall allow apex-remote
```

which is root, and is a separate decision because the relay path works without
it. A machine with the port shut is still reachable through a relay; a machine
with it open is reachable directly on the LAN, which is faster.

## `apex remote pair`

Shows a pairing code. It is good for three minutes and pairs exactly **one**
device, and it carries this machine's public key — so the device that scans it
can never be talked into trusting a different machine.

**This build prints no QR code.** No encoder is vendored, and a wrong QR is
worse than none: a phone scans it, fails, and the person concludes their camera
is broken. What the terminal prints is the payload itself, an `apex-remote:`
URI whose body is base64url of the compact-JSON offer, together with a line
saying where a real QR code will come from (APEX Settings, which has a
renderer). Paste the line into APEX Remote on the phone.

`--text` prints the payload and nothing else **on stdout**; the prose — how
long it is good for, and the sentence about the public key — goes to stderr. So

```
apex remote pair --text > offer.txt
```

leaves `offer.txt` holding exactly one line, the `apex-remote:` URI, with the
explanation still on the terminal. That split is what makes the verb scriptable
without a second output mode.

There is deliberately **no `apex remote pair --json`**. The payload is already
a single self-describing string; wrapping it in an object would be a second
shape to keep in step with the phone for no reader that does not already have
the string.

## `apex remote devices`

Every device this machine has paired, and which of them are revoked:

```
ID                 NAME                 STATE     PATH     LAST SEEN
9f3c1a…            Pixel 8              paired    lan      4m ago
```

`--json` emits the stored `Device` records, which carry:

| field | meaning |
| --- | --- |
| `id` | short, stable, **derived from the public key**, so two records for one key are impossible |
| `name` | what the owner called it; display only, so a duplicate is untidy rather than dangerous |
| `public_key` | the authenticating fact — base64url, no padding |
| `paired_ms` | Unix ms at which pairing completed |
| `last_seen_ms` | Unix ms of the last completed handshake, or absent |
| `revoked_ms` | Unix ms at which the owner revoked it, or absent |
| `requires_user_verification` | whether this machine requires the device to have re-authenticated its user |
| `last_path` | the transport the last completed handshake arrived over |

The `STATE` column is **derived from `revoked_ms` alone**: `revoked` if it is
set, `paired` if it is not. There is no third state and no stored status field
that could disagree with the timestamp.

`requires_user_verification` is reported once, in a sentence at the end, and
deliberately not as a column. The desktop cannot see a fingerprint; what the
flag buys is that the device's key is held in a keystore that will not sign
without a biometric or device credential. That is the **device's own claim**,
and a tick in a table would read as a fact this machine had verified.

## `apex remote revoke <device>`

Takes a device's access away. The argument is a device id, or its name when
that names exactly one.

Immediate: a connection the device is already holding is dropped, not merely
refused next time. The record is kept rather than deleted, so the listing can
still show that the device was revoked and when.

## `apex remote status`

Whether the service is running, and how a device would reach it — the machine
name, the protocol revision, this machine's identity key, how many devices are
paired, the LAN addresses, the relay and rendezvous if one is configured, and
any pairing code still open.

It also lists **every connection open right now**: which device, which path it
came in on, how good that path is as a word, and the round trip in
milliseconds. The number is for whoever wants it; the word is what tells
somebody whether their terminal is going to feel wrong. A relay path prints its
disclosure — what the relay can and cannot see — rather than leaving the reader
to assume.

`--json` is how APEX Settings reads the same measurement: `machine`,
`protocol`, `identity`, `paired`, `lan`, `relay`, `rendezvous`,
`offer_ms_left`, and `connections[]` with `device_id`, `device_name`, `path`,
`since_ms`, `rtt_ms`, `quality` and `disclosure`.

## When the service is not running

Every verb that talks to `apex-remoted` fails with the remedy rather than a
connection error: it names the socket it tried and tells you to run
`systemctl --user enable --now apex-remoted` — which is what `apex remote
enable` does for you.

## What a paired device may ask, and the one thing it may not

`apex-remoted` is a forwarder, not a filter. Its `control()` refuses exactly
two verbs on the control channel — `attach` and `receive`, because both take
the connection over and would wedge it — and hands everything else to
`apex-agentd` under the origin `claude-remote-control`. So what a phone can do
is decided by `apex-agentd`'s own per-verb rules, in one place, and not by a
denylist here that would drift from them.

Two verbs exist for the phone's pickers, and they are deliberately cheap:

| verb | what it answers | cost |
| --- | --- | --- |
| `projects` | every project the runtime remembers: root, name, slug, detected toolchains, when it was last opened, and the capsule (§8) it is bound to | one small JSON record per project; no subprocess |
| `profiles` | one row per adapter: its program, whether that program is on **this machine's** `PATH`, whether the adapter needs a program named by the caller, and whether its profile (§5) is installed here | a `stat` per table entry and per `PATH` element |
| `worktrees` | per-worktree git status for every remembered project, or for one slug | runs `git worktree list`, a rev walk and `merge-tree --write-tree` in each project — seconds on a large machine |

A client builds a picker from the first two and asks the third only for the
project the user chose. That split is the reason they are separate verbs.

`projects` and `profiles` carry **counts and paths the machine already
publishes through `worktrees`, and no content**. A profile row never carries
the configured model, a plugin, a marketplace, an MCP server, a skill or a
credential — `apex agent profile doctor` has all of that and none of it crosses
this wire. `secret` is a count of credential-class entries that exist, never a
name and never a value.

**`decide` is refused from any origin that is not local, and that is the
design.** Every verb in the privilege vocabulary is a root capability
(`request.rs`'s `Verb::capability`, all eight), so §7's row for both columns
ends at a human at this machine. The check sits *before* the pending lookup, so
a paired device is told `permission_denied` whether or not the request id
exists — the refusal cannot be used to discover which requests are open on
somebody's computer. A phone can see pending requests, see what was decided,
and **revoke** a grant of either kind; revoking carries no origin check,
because revocation only ever removes authority.

The exception that exists is a different verb with a different shape.
`renew_system_grant` accepts a non-local caller when the owner has written
`origin = remote_elevation_allowed`, and what that opt-in costs is a
`second_factor`: a WebAuthn assertion from a key the owner enrolled with `apex
agent key add`, over a nonce the daemon issued and bound to that one session,
grant kind and window. Remote approval of a root operation, if it is ever
built, has to be that shape and not a relaxed origin check.
