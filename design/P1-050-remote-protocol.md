# P1-050/051/052: the APEX Remote protocol, and what it refuses

Written against the code on `task/p1-050-remote-protocol` in apex-os, forked
from `roadmap/v2.2` @ `bc4f06e`. Every claim about existing behaviour was read
out of the source or produced by running it; the two places where it was
produced by running it are marked.

This note is meant to be disagreed with. The three decisions most worth
arguing about are in **What I decided not to do**, and the one that matters
most is the first.

## The shape, in one paragraph

A paired device opens one connection. Inside it, a Noise channel; inside that,
multiplexed frames; inside the control frames, **`apex-agentd`'s own protocol,
byte for byte**. `apex-remoted` is a proxy that terminates the channel,
authenticates the device from the handshake, and forwards each control frame
on its own connection to the agent runtime — declaring `claude-remote-control`
on that connection before it forwards anything. The device never says who it
is; the handshake already did.

```
phone ── Noise_IK ──> apex-remoted ── unix socket ──> apex-agentd ──> PTY
             │              │                  │
             │              │                  └── first line on every
             │              │                      connection is
             │              │                      DeclareOrigin
             │              └── holds no session, owns no PTY, has no
             │                  privileges of its own
             └── LAN direct, or the same bytes through a relay
```

## Why the control payload is the daemon's own protocol

This is the load-bearing decision and it is what makes "provider-neutral"
true rather than aspirational.

`apex-agentd` already launches Claude, OpenCode, Codex, Gemini and a generic
managed PTY through one adapter layer, and already reports project, worktree,
checkpoint, the six permission dimensions and the request origin in one
`SessionInfo`. A remote protocol that re-described any of that would be a
second vocabulary to keep in step with the first, and the second one always
falls behind — usually within one release of the thing it was mirroring.

Three consequences, and the third is the one that pays for the decision:

- **No screen scraping**, which P1-050 asks for in as many words. The phone
  gets the same typed records the Agent Center does.
- **Provider neutrality is inherited.** Nothing in `apex-remote-core` names an
  agent. Adding one is an adapter in the runtime, and the phone gets it.
- **A new daemon verb reaches a phone for free.** P1-020's agent graph is
  being added to that protocol on another branch right now, and not one line
  here has to change to carry it.

The cost is real: this layer cannot check what it forwards. It is not supposed
to. The daemon does the checking, which is why the origin declaration is the
first thing on every proxied connection and why a refused declaration is fatal
rather than a warning.

## How origin survives the hop, and the hole it was hiding

**This is what I found that the roadmap did not know.**

P0-013/014 established request origin from the *connection's own provenance* —
`SO_PEERCRED`, then `/proc/<pid>/cgroup`. `Request::DeclareOrigin` let a
managed session narrow its own record, and refused any connection that was not
a session:

> "only a managed session can narrow its own origin; this connection is not
> one"

A proxy is never a session. So the origin of everything `apex-remoted`
forwarded would have been whatever `origin::classify` made of `apex-remoted`'s
own cgroup:

| how the proxy was started | cgroup | observed origin | may approve root? |
|---|---|---|---|
| `systemctl --user` | `user@1000.service` | `scheduled-job` | no |
| from a terminal | `session-3.scope` | `local-terminal` | **yes** |
| launched by the shell | `session-3.scope` | `apex-shell` | **yes** |

`privilege::decide` gates approving a root operation on exactly
`origin.is_local()`. So on the second and third rows, a phone could have
approved a root operation by asking a proxy that happened to be started the
obvious way — and the daemon would have recorded it as a human at the
keyboard. Nothing stood between the two. The safe row is safe by accident:
nobody chose `scheduled-job` for a remote proxy, it is simply what a user unit
classifies as.

The fix is a **per-connection origin latch**. A connection that is not a
session may declare a narrower origin once; it sticks for the life of the
connection and dies with the socket. The check is `origin::may_declare`,
unchanged, and it was already total over the 7×7 product: a declaration may
only ever cost the connection something, and the two local origins are refused
by name whatever was observed. So the latch cannot launder remote into local
even if `apex-remoted` were compromised and tried.

Two details worth arguing with:

- The latch is applied **where it is used**, not only where it was accepted.
  `privilege::origin` re-derives it from the live observation on every
  request, so a narrowing that has stopped being a narrowing simply stops
  applying and the observation stands. The alternative — check once, trust a
  stored value — moves the check away from the thing it protects.
- A declaration that *would loosen* is ignored rather than fatal. Refusing
  outright would let a client kill its own connection by asking for something
  it was already going to be denied, and the fail-safe direction is the
  observation.

There is a second guard, deliberately: `apex-remoted` refuses to start unless
its own cgroup says it is a systemd user service. The latch is a runtime
discipline; this is a fact about how the process was started. One guard for a
hole this shape is not enough, and `--allow-foreground` says what it turns
off.

**Does this close P0-014?** No, and see below.

## Device identity

`DeclareOrigin` gained an `actor` field: the paired device's id, recorded on
sessions and on privilege requests beside the origin. `request_origin` says
*what kind* of thing is asking; `actor` says *which one*. Without it an audit
trail can answer "was this remote" and not "was this my phone", and the second
is the question somebody actually asks.

It is bounded at 64 characters and refused for control characters, because it
is printed on the prompt a human reads before handing out root. A device
called `phone\r\nAPPROVED` is a display attack, and the same rule already
applies to package names in `request.rs`.

Not a `PROTOCOL_VERSION` bump. The file's own rule is that a bump is for
changes where an old daemon dropping a key loses a *restriction* — and a
daemon below this **refuses** the declaration with `permission_denied` rather
than dropping it, so a proxy that needs one fails loudly instead of forwarding
under a local origin. The key it does drop is a display string.

## Pairing

The QR code is the security boundary, and that is the design rather than a
weakness of it. It is an out-of-band authenticated channel: an attacker on the
network cannot write to it, and one who can read it is standing in the room.
The code carries this machine's static X25519 public key and the device pins
it. No CA, no trust-on-first-use, no "compare these emoji" step that people
click through. A man in the middle presenting its own key makes the handshake
**fail**, not succeed with a warning.

The public key authenticates the desktop to the device. The one-time token
authenticates the other direction: it says the device holding it is the one
the owner just showed the code to, and not a second device that learned the
key from a photograph of the screen an hour earlier. Random 32 bytes,
single-use, three minutes, compared in constant time, and never outside the
encrypted handshake.

Two handshakes:

| when | pattern | why |
|---|---|---|
| pairing | `Noise_NK_25519_ChaChaPoly_BLAKE2s` | the device knows the desktop's key from the QR; the desktop knows nothing about the device |
| afterwards | `Noise_IK_25519_ChaChaPoly_BLAKE2s` | both statics known; IK sends the device's key **encrypted**, so a relay never learns which device is connecting |

`XX` is used nowhere. "The device need not know who it is talking to" is
precisely what makes a man in the middle possible.

**"Pairing cannot be completed silently by an agent"** (P1-051's fourth
criterion) has two independent mechanisms, because one is a policy and one is
a fact:

- An offer exists only while a human asked for one, is single-use, and expires
  on its own. There is no verb that creates a token as a side effect.
- `apex-remoted` refuses `pair` for any caller whose observed origin is not
  local — the same `observe_pid` rule `privilege::decide` uses. An agent
  inside a managed session is under `user@N.service`, classifies as
  `scheduled-job`, and cannot move itself.

Listing and revoking are **not** gated that way, on purpose. An agent that
revokes the owner's phone is a nuisance, not an escalation, and the owner is
the only one who can pair it back. Refusing a revoke from a script would mean
the response to a lost phone is slower than the response to a lost laptop.

Revocation is a tombstone, not a deletion, and it is immediate: the connection
the device is already holding is `shutdown(2)`'d, not merely refused next
time. A flag the connection thread checked would do nothing until the phone
next sent a byte, and a phone in somebody else's pocket may not send one for
hours.

## What the relay operator can see

Stated plainly, because a design note that says "end-to-end encrypted" and
stops there is hiding the interesting half.

The relay copies bytes. It terminates no encryption and holds no key: the
Noise channel is established end to end *through* it. It carries ciphertext.

**It can see:**

- **The rendezvous id.** Both ends must name the same meeting point, so the
  operator necessarily learns it. It is a domain-separated SHA-256 of the
  desktop's static public key, truncated to 128 bits — never the key. It is
  stable, so the operator can tell that the same desktop is being reached
  today and last week.
- **Both IP addresses**, and therefore roughly where the phone and the laptop
  are.
- **Timing and volume**: when a session is open, how long for, how many bytes
  went each way, in what bursts. That is enough to infer typing rhythm,
  roughly how much output a command produced, and when somebody is working.
  PTY traffic is especially leaky here — a keystroke is one small frame.
- **That this is APEX Remote traffic**, from the path shape.

**It cannot see:** the content of any terminal, prompt, file, project name or
session id; any device name; or **which device** is connecting — the device's
static key travels encrypted inside the `Noise_IK` handshake, so it is not a
correlation handle the way the rendezvous id is.

Why the rendezvous id is a hash and not the key: publishing the key as the
meeting-point name would hand every relay operator, and everyone who can watch
one, the exact value a device pins during pairing. It would not let them
decrypt anything. It would let them *offer* that key to a device — and a
device that accepted a key it had not seen on a QR code is a device with no
pairing security at all. Hashing costs nothing and removes the temptation.

An owner unwilling to give up the traffic-analysis metadata should run
LAN-only, which is a complete configuration: leave the relay unconfigured and
the QR code carries no relay at all.

## The firewall, decided deliberately

APEX ships default-drop. Only ssh and mDNS are open.

- **The relay path needs no exception at all.** It is an outbound connection
  from both ends, which is also what "no inbound router port forwarding"
  means. This is the better default off-LAN and it costs the user nothing to
  arrange.
- **The LAN path needs one**: `apex-remote tcp 7717`, in the catalogue,
  **closed** until `sudo apex firewall allow apex-remote`. `apex remote
  enable` says so and does not do it. The image asserts both that the entry
  exists and that nothing has put it in the nftables policy.

Opening a port because a feature exists is how a default-drop policy becomes a
list of things somebody once wanted. The cost of the decision is one command
and one sentence of explanation; the benefit is that "what is open" stays a
record of what the owner allowed.

## What secrets do, and do not, cross

Nothing had to be built for this and it is worth saying why.

P0-002 made `SecretValue` implement neither `Serialize` nor `Deserialize`,
and `Response` derives `Serialize` — so a daemon reply carrying a credential
**does not compile**. What this protocol forwards is that same `Response`. The
property is inherited, and it is a type-system property rather than a filter
this proxy could get wrong.

The honest exception: **PTY bytes are not filtered.** A secret an agent echoes
into its terminal reaches the phone exactly as it reaches a local terminal.
That is the same exposure a local `apex agent attach` has, and filtering a
terminal stream for secrets is not a thing that works.

## Does P1-051 give P0-014 a real second factor?

**No. It gives the shape of one, and `--origin-policy remote` should stay
refused.**

What pairing provides: a per-device asymmetric keypair, possession of which is
proved by every handshake, revocable from the desktop, with a
`requires_user_verification` flag the owner can set.

What is missing, in the order it has to be fixed:

1. **The flag is a claim, not a fact.** The desktop cannot see a fingerprint.
   It becomes real only when the device key lives in Android Keystore with
   `setUserAuthenticationRequired(true)`, so the *key will not sign* without a
   biometric or device credential. That is Android work — P1-053 and P1-057 —
   and until it lands, `requires_user_verification` is recorded and displayed
   as the device's own claim. `apex remote devices` says so in those words.
2. **There is no per-approval challenge.** A session-long channel proves the
   device was present when the channel opened, not when the approval happened.
   A real second factor needs the desktop to demand a fresh signature over the
   specific request id, at the moment of approval, from that key.
3. **There is no attestation.** Nothing proves the key is in hardware at all,
   which is what makes FIDO/WebAuthn's claim different in kind. Android key
   attestation could close this and is not built.

So this branch does not wire pairing to `OriginPolicy::RemoteElevationAllowed`
and `AgentPolicy::validate` still refuses it. Claiming otherwise would turn a
named P0 remainder into a P0 that looks closed.

There is one more gap, which the roadmap has already accepted elsewhere and
which is worth naming here because pairing makes it concrete: an unmanaged
`claude` running in a plain terminal with Claude Remote Control switched on
classifies as `local-terminal` and could therefore run `apex remote pair`.
That is the same hole Claude Remote Control has everywhere in this design —
the kernel cannot see who is typing — and it is not solved here.

## What I decided not to do

**An eighth `RequestOrigin`.** APEX Remote is not Claude Remote Control and
the wire name it reuses is `claude-remote-control`. This is the decision most
worth arguing with. Against reusing it: the name is now a lie about the
provider, in a field whose whole purpose is to be read by a human deciding
about root. For reusing it: an eighth origin is a `PROTOCOL_VERSION` bump
whose failure mode is a daemon dropping the unknown value and recording
`scheduled-job` — which loses the lock gate, the exact failure shape
`REQUEST_ORIGIN_VERSION` exists to catch; it changes `Capability::ruling`, the
7×7 declaration test and P0-014's table; and it conflicts with an open branch
on the same file. The semantic §7 actually needs is "a human elsewhere is
driving this machine", and both providers are that. `actor` carries which
device far better than an origin name would. If this is wrong, the fix is a
rename of the variant's serde tag with a migration, not a new variant.

**A second permission system.** P1-057's remote capability approvals should be
P1-001's vocabulary reaching further, not a parallel one. Nothing here models
a capability, a grant or an approval; a phone approving something is a phone
sending `decide` and the daemon applying §7's table to the origin it recorded.
Today that means root approvals are refused from a phone, which is correct and
is what P1-057 will have to argue with rather than around.

**Screen-scraping anything, or defining a "session summary" record.** The
temptation is a compact record shaped for a phone screen. It would have been
out of date within a release.

**A QR encoder.** None is vendored, and `apex remote pair` prints the payload
and says so rather than drawing something wrong. A phone that scans a broken
QR leaves the person blaming their camera. P1-051's first criterion says the
code comes from APEX Shell, which has a renderer — that is where it belongs
and it is a named remainder.

**mDNS discovery.** The QR carries this machine's LAN addresses, so a direct
LAN connection works with no cloud dependency, which is P1-052's first
criterion. What is missing is *re-discovery* after the laptop's address
changes. mDNS is already open in the firewall, `_apex-remote._tcp` is the
obvious record, and the cheap implementation — a static Avahi service file —
would advertise the machine whether or not the service is running, which is a
worse answer than none. Named as a remainder rather than half-built.

**Trusting the supply chain.** Nothing on any APEX machine verifies an image
signature and `/etc/containers/policy.json` holds one
`insecureAcceptAnything`. So the identity key is a file at 0600 in a 0700
directory — the same protection the privilege request store has — and not
something claiming a hardware root of trust this platform has not got. A
remote attacker never sees it, because it never travels. Anything running as
this user can read it, and the note says so rather than implying otherwise.

## Where the remainder is

| criterion | state |
|---|---|
| P1-050 protocol, desktop service, origin + device identity | done |
| P1-050 "tasks, diffs/tests, checkpoints" as first-class verbs | carried, not modelled — they are the daemon's verbs, and P1-036/P1-020 own the ones that do not exist yet |
| P1-051 pairing, revocation, device list | done |
| P1-051 QR from APEX Shell, revoke from APEX Settings | apex-shell work, not started |
| P1-051 biometric enforcement | flag recorded; the enforcement is Android |
| P1-052 LAN direct | done, via addresses in the pairing payload |
| P1-052 LAN discovery | not done — see above |
| P1-052 relay | the client shape, the rendezvous derivation and the disclosure are here; no Worker is deployed and no reconnect state machine is written |
| P1-052 reconnect across network changes | not done |
| P1-052 connection quality visible | `Ping`/`Pong` frames exist and carry a token; nothing renders the round-trip yet |
