# Rime Fleet: a design, and nothing else yet

Roadmap P2-019. This document is the deliverable: an architecture for managing
many Rime machines that scales down to one, written against what the system
already does instead of the shape a fleet product usually takes.

**No code here exists.** There is no `rime fleet` verb, no enrolment record, no
server. Every mechanism named below either already ships and is cited, or is
marked as not built. Every section keeps that distinction, because a design
document that reads as a feature list is how a roadmap item gets recorded as
done twice.

Read this before the rest: one piece has left this document. The
staged-rollout pointer (the percentage that has to change between builds) was
going to be a fleet endpoint, because an OCI label is baked once and a fleet is
the obvious thing that could serve a number. It is now a signed document in the
registry that **every** machine reads, enrolled or not, and it ships. Designing
it as a fleet feature would have made a personal machine's update path worse in
order to give a managed one something. That is the failure the rule below
exists to prevent, and this design nearly reached it by accident.

## The rule that comes before the architecture

> Enterprise/fleet support remains optional and does not turn personal Rime
> installs into managed devices by default.

That is the third acceptance criterion, and it comes first on purpose: it
constrains the shape of everything else, and no setting can stand in for it.

The mechanism: **an unenrolled machine has no fleet code path to disable**. A
machine joins a fleet by an explicit, authenticated act that writes one record.
With no record, the client is inert: it starts no timer, opens no socket, and
contacts nothing. "Inert" has to be measurable, and this repository measures
that kind of claim with a test that asserts the absence.
`tests/test-rime-channel.sh` already proves `rime channel report` sends nothing,
by printing the payload and asserting nothing left the machine, and
`docs/update-channels.md` puts it in the plainest sentence in the tree:
*"Nothing, to nobody. Rime operates no telemetry service."* A fleet client must
keep that sentence true for an unenrolled machine, and its test must fail if
the sentence stops being true.

The second half of the rule: **unenrolling works from the machine**. A device
whose owner cannot leave a fleet from the keyboard is a managed device sold as
a personal computer that opted in. A fleet may refuse to *re-enrol* a machine
that left, and it may report that it left. It may not hold it.

## What already exists, per machine

Nothing in this table is proposed. It is the substrate, and the design's main
claim is that a fleet is a distribution problem on top of it, not a new agent.

| need | what ships today | where |
| --- | --- | --- |
| declarative state | `rime blueprint diff/apply`, idempotent, secrets never in the bundle | `rimed-core/src/blueprint.rs`, BASE-007 |
| update rings | `edge`/`beta`/`candidate`/`stable`, promotion gated in CI on a cosign signature | `rime channel`, P1-046 |
| staged rollout | a stable slot 0–99 per machine, and a signed rollout document in the registry that says how far the ramp has got | `rime channel rollout`, P1-046 |
| a health verdict | four rows that count, measured by the same probes as recovery | `ops::update`'s rollout stop |
| inventory facts | channel, tag, digest, healthy, reasons | `rime channel report` |
| device identity | a long-lived key, and paired devices | `rime-remote-core::identity`, `rime remote` |
| credentials | root-owned per-uid store, no verb returns a value | `rime-secretd`, P0-002 |
| capability grants | per project, per operation, spendable by the daemon only | P1-001 |
| supply chain | SBOM, provenance, cosign verification | `rime provenance`, `rime trust`, P1-047 |
| recovery | previous deployment, rollback, doctor, Safe Graphics | `rime recover`, `docs/recovery.md`, P2-018 |
| privilege | seven request origins, `cloud-job` among them, declared not observed | `rime-agent-core::origin`, §7 |

Fleet designs most often get the last row wrong. Rime already has a vocabulary
for "this request came from somewhere other than a human at this keyboard", and
`cloud-job` is one of its values. A fleet is a *remote origin*. It needs no new
privilege model; it needs to declare which origin it is.

## Enrolment

**Not built.** The shape:

```
rime fleet status                 # unenrolled, and what that means
sudo rime fleet join <token>      # the one act that changes anything
sudo rime fleet leave             # from this keyboard, always
```

A join token carries the fleet's identity and an endpoint. It is single-use and
short-lived. The machine already has a long-lived key pair
(`rime-remote-core::identity::Identity::load_or_create`), and the enrolment
exchange should be the pairing exchange Rime already performs for a phone: the
same Noise handshake, the same store, a different peer role. A second
device-identity mechanism would be a second place for a key to be wrong.

The exchange needs three properties, each learned the hard way in this
repository:

* **The token proves who the operator is, as well as where to connect.** An
  attacker can mint a token that only says where to connect by standing up a
  server.
* **Enrolment is refused from inside a managed agent session.** `rime-secretd`
  already refuses `Add`, `Remove`, `Grant` and `Approve` from a caller whose
  cgroup or process ancestry says it is inside `rime-agentd`
  (`rime-secretd/src/main.rs::refuse_a_session`). Joining a fleet is a larger
  act than granting a capability and gets at least the same gate.
* **Every verb is checked, including the dull ones.** P2-016 found
  `rime-remoted` checking the caller's identity for `Pair` and for nothing else,
  so a second local account read the owner's machine key out of `Status`, the
  paired-device list out of `Devices`, and reached the device store through
  `Revoke`. A fleet client has more verbs than that and the same failure mode.
  Authorisation belongs in a `match` over every request variant, with a
  table-driven test that names each one;
  `rime-remoted/src/control.rs::authorized` is the pattern.

Enrolment writes one record under `/var/lib/rime-fleet/`, root-owned, `0700`:
the fleet id, the endpoint, the operator's public key, and the time. Nothing
else in this design runs unless that file exists.

## The transport

**Not built, and now decided.** The client **polls** over ordinary HTTPS, and
`relay/` is not in the fleet path.

The action list settles this, not a taste for polling. Read *What must never be
built* first: no remote exec, no fleet-side approval, no silent enrolment. That
leaves a fleet three things it may do to a machine: set its channel, set a ring
ceiling, and hold its updates. Only the last is ever wanted in a hurry, and none
of them needs sub-minute latency. **A transport chosen for push latency would
buy speed for actions this design does not permit**, and would charge for it
twice:

* A machine that holds a connection is continuously reachable. An unenrolled
  machine has no fleet code path to disable *because there is no socket*: the
  shape of the client keeps the rule that comes before the architecture, and no
  flag can switch that off.
* A relay that carries fleet traffic learns which machines are awake, and when.
  That is a continuous liveness map, the class of data
  `docs/update-channels.md` promises not to collect, gathered through a side
  door. `relay/` was written for a phone talking to its owner's desktop, where
  both ends belong to the same person. A fleet relay is a third party watching,
  and "it only sees ciphertext" does not answer that.

The machine's own timer sets when it talks, and nothing reaches it in between.

### What a poll is

A poll has the shape of `rime channel report`, which already composes a payload
the machine sends and can print instead. Out goes the inventory row; back comes
a **document, never a command**: channel, ring ceiling, a hold flag, a policy
body. Everything the client may do with it is something a local verb already
does. A response that named a command to run would be remote exec through a
data channel: item 1 of the list this design refuses, wearing a different hat.

### When a poll happens

On a jittered interval derived from the **stable slot 0–99** that `rime channel
status` already computes from `/etc/machine-id`. The reuse is deliberate:
staging a rollout and staggering a poll both need a stable per-machine number in
the same range that nobody has to configure, and ten thousand machines polling
on the hour is a self-inflicted outage.

The cost: **"hold this machine's updates" takes effect at the next poll.** An
operator gets no answer faster than the interval, and no answer at all from a
machine that is off. Both are acceptable for the three actions above. Neither
would be if remote exec were on the list, so the transport and the permission
list have to be chosen together.

### What makes a response trustworthy

The signatures, not the transport. The machine authenticates the poll with its
long-lived key (the identity enrolment already used, not a second one), and the
**operator's key recorded at enrolment signs the response**. A hostile network,
a compromised CDN or a misissued certificate can then deny service but cannot
change a machine's channel. The signature carries the trust; TLS is a
convenience.

A signature alone does not give two things, and both have to be in the document
itself:

* **Replay.** An old, validly signed "you are on `edge`" is a downgrade attack
  unless the document carries the machine id and a monotonic counter the client
  refuses to go backwards on.
* **Expiry.** With no freshness bound, a fleet that stops answering pins every
  machine to its last instruction, and nothing says so. The document should
  expire, and an expired document should leave the machine on its own local
  configuration, not on the fleet's last word.

## Inventory

**Partly built.** `rime channel report` already computes the payload and already
refuses to send it. A fleet's inventory report is that payload plus what an
operator cannot get any other way:

```json
{
  "machine": "<fleet-assigned id, not /etc/machine-id>",
  "channel": "edge",
  "tag": "daily",
  "digest": "sha256:…",
  "healthy": true,
  "reasons": [],
  "deployments": ["<booted>", "<rollback>"],
  "extensions": ["…"],
  "failedUnits": ["…"]
}
```

Two decisions in that object:

**The machine id sent is not `/etc/machine-id`.** The rollout slot comes from
it, and `rime channel status` already says the slot "is derived from this
machine's id and is never sent anywhere". A fleet needs a stable handle, but not
that one: sending it would turn a value the machine keeps to itself into a
fleet-wide correlator.

**There is no user in it:** no account names, no home directory sizes, no
record of who installed what. An operator who manages a device does not thereby
manage the person using it, and a report that carries the person is the report
that gets a fleet product banned from a school. If a future criterion needs
per-user facts, they get their own consent surface and their own section.

## Compliance

**Partly built. This section has the least new work in it.**

Rime already computes, on the machine, everything a compliance check would ask
for, in a form with stable identifiers:

* `rime recover status --json`: eight rows with stable ids
  (`current-deployment`, `previous-deployment`, `secure-boot`, `filesystem`,
  `gpu-driver`, `rime-shell`, `network`, `package-extensions`). The ids are
  already a compatibility surface: rime-shell's `RecoveryService` keys on them.
* `rime doctor --json`: the full health report.
* `rime trust` and `rime provenance`: signature and SBOM state.
* `rime channel status`: which ring, and whether this machine is behind it.

A compliance policy is therefore a **predicate over facts that already have
names**, not a new agent that re-measures the machine. A second measurement path
would drift from the first, and the machine's own report is the one the user
can run.

A policy must never be a *remediation with root*. A fleet may say a machine is
out of compliance and may refuse it a resource; it may not repair it behind the
user's back. Repair is `sudo rime …` run by somebody who can answer for it, and
Rime's polkit actions are `auth_admin` with `allow_inactive=no` so that somebody
has to be at the keyboard (`docs/multi-user.md`).

## Update rings

**Mostly built, and the missing piece is already written down.**

A ring is a pair: a channel, and a ceiling on the rollout slot. The channel half
ships (`rime channel set`, promotion gated in CI on a cosign signature from this
repository's `main` workflow). The slot half ships (`0–99`, stable across
reboots, derived from `/etc/machine-id`).

The pointer did not ship when this section was first written, and
`docs/update-channels.md` said why:

> nothing publishes a percentage yet. The gate reads it from an image label and
> treats an image without one as reaching everybody… A ramp — 1%, then 5%, then
> 25% — needs the number to change between builds, and a label is baked once.

**That quotation is out of date, and how it was resolved matters more than the
resolution.** The obvious conclusion from "a label is baked once" was "so a
staged rollout needs a fleet endpoint", and that would have made the ramp a
feature only managed machines get: the shape the rule at the top of this
document exists to refuse.

The pointer went somewhere else: a **signed rollout document**, published at
`…:rollout` in the same registry as the image and re-published whenever the
number moves. It is mutable, it needs no server, and a personal machine reads it
with the cosign verification it already performs on the image. It ships:
`docs/update-channels.md` describes it, and
`rimed_core::channel::decide_rollout` is everything a machine does with one.

The ring is therefore:

```
ring := { channel, max_slot, halt }
```

**The fleet's half of it is now only the ceiling**, because everybody receives
the other two:

| part | who serves it | to whom |
| --- | --- | --- |
| channel | the tag in the machine's bootc origin | everybody; `rime channel set` |
| the ramp | the signed rollout document | everybody, per channel |
| `halt` | the same document | everybody, per channel |
| `max_slot` | a fleet | its enrolled machines only |

A fleet therefore adds one narrow power: **it can hold its own machines further
back than the publisher's ramp, and it can never push them ahead of it.** The
decision function enforces that, not this document:
`RolloutQuery::fleet_ceiling` only ever lowers the percentage, and a test fails
if a ceiling above the ramp raises it. Every machine passes `None` today,
because no fleet client exists.

`halt` left the fleet's hands for the same reason the ramp did, and there are
now three stops. The per-machine stop that already ships refuses an update when
*this* machine came back unhealthy from the last one. The publisher's halt, in
the rollout document, stops a digest reaching anybody. The third, which would
fire when enough machines in a cohort came back unhealthy, does not exist in
this design, and the transport is not what is missing: counting a cohort needs
machines to report health to their fleet, which is the reporting under *Remote
health and recovery*, is opt-in, and has no receiver. The halt that ships is
therefore a person's judgement, sent after reading evidence from somewhere else.
The automatic one stays on the not-built list instead of being written up as
though only the wiring were missing.

Whichever of the three fires, the machine's own stop stays authoritative for the
machine: a fleet may hold a rollout, and may not force one past a local refusal.

## Managed secrets and certificates

**Not built, and the hardest section.**

`rime-secretd`'s store is **per uid** (`/var/lib/rime-secretd/users/<uid>/`),
and authorisation depends on that: it is `SO_PEERCRED`'s uid, and no verb names
another account. A fleet secret belongs to the machine, not to a uid.

The design is a second namespace under the same daemon, not a second daemon:

* `/var/lib/rime-secretd/machine/`: root-owned, `0700`, alongside `users/`.
* **No local account can read** a machine credential. The daemon spends it as
  it spends a user's, and the same compile-time property holds: `SecretValue`
  has no `Serialize` impl and no `Response` variant can carry a credential.
* What a local caller may ask for is a *capability*, and which capabilities a
  fleet credential backs is part of the enrolment record, not of a user's grant
  table.

Certificates are the easier half and should not be folded into the harder one.
A fleet's CA bundle and its 802.1X client certificates are files with owners and
expiry dates, not brokered operations. They belong in the image's trust
configuration with a fleet-managed drop-in. The property to preserve: a machine
that leaves a fleet stops trusting the fleet's anchors, so the anchors go in a
directory `rime fleet leave` empties, not in the base trust store.

The need is real. An 802.1X profile that validates no CA certificate
authenticates the client to the network and the network to nobody, and profiles
in that state are common on machines joined to a school or office network by
hand. A fleet that distributed an anchor and pinned it would close a real hole.

## App policy

**Partly built.** The pieces:

* `rime blueprint` describes desired apps and applies idempotently, and
  BASE-007 proved its sync bundle carries no secrets (a sentinel token planted
  in the old broker's store, absent from the exported bundle).
* `rime-perm-core` reports what an application holds (portals, Flatpak context,
  device ACLs) without inventing a second enforcement layer (P1-061).
* Plugin trust decides whether third-party code may run at all (P1-025).

A fleet app policy is a blueprint the machine did not write, plus a deny list,
plus the rule `rime-perm-core` already follows: **report what is true, and do
not claim what is not enforced.** A policy that says "the camera is disabled"
when the mechanism is a Flatpak override a user can remove lies to the
operator. The crate reports and adds no enforcement of its own, and the fleet
layer should follow the same rule.

## Remote health and recovery

**Partly built.** An unwell machine is the one least able to tell anybody, which
is why the recovery work sits under P2-018 and this section is short.

* The per-machine half exists: `rime recover status`, `rime doctor`,
  `sudo rime rollback`, and Rime Safe Graphics, which comes up on the CPU with
  its own compositor configuration when the normal desktop does not paint.
* The fleet half *reports* that state and does not drive it. An operator can
  see that a machine rolled back and why. An operator cannot roll a machine back
  remotely in this design, for the reason given under Compliance: a deployment
  change is `auth_admin`, `allow_inactive=no`, answered by somebody at the
  keyboard.

The one remote action worth designing is the narrow one: **hold this machine's
updates.** It is reversible, it cannot brick anything, and it is what an
operator wants in the hour after a bad release.

## The server side

**Not built, and not part of Rime by design.** Rime ships a client and a
protocol. A fleet that worked only against one hosted server would make
"optional" untrue for anybody who cannot or will not use it, so the server is a
separate deliverable, and this section describes what it has to be, not what it
is.

### The smallest correct server is a signing tool and a bucket

Because a poll response is a signed document and nothing else, the minimum
viable fleet server has **no always-on service and no database**: one signed
document per machine, served as a static object. An operator with fifty
machines can run a fleet out of object storage and a script. That version
serves the same artefact the large version serves, so keeping the promise that
personal installs are not managed devices costs an operator nothing.

The static version stops at **group membership**: deciding which machine gets
which document wants a database once the answer is not "one file per machine
id". A few thousand machines is the realistic limit of the static version.

### Tenancy is key separation, not rows

**The unit of authority is a signing key, not a login.** A fleet *is* a key
pair; a machine enrols to a public key; two fleets are two keys. A server that
separated tenants only by partitioning rows would have one place where every
tenant's policy could be rewritten, but because the client checks a signature,
that server could not rewrite anything. Cryptographic tenancy fails safe where
administrative tenancy does not.

The operator side still has to support two things, and each one shapes the
machine's half:

* **More than one human, and a record of who signed what.** The machine
  therefore records the fleet's **root** key at enrolment and accepts a document
  signed by a delegated key whose delegation chains to that root: the shape
  `rime trust` and P1-047's cosign verification already use, not a second one.
* **Revoking a signer without re-enrolling every machine.** This is the same
  requirement from the other end, and the reason enrolment pins the root key.

### Read is a server permission; write is a cryptographic one

The only server-side action that changes a machine is moving it between
documents, and that needs the signing key. Everything else an operator console
does (listing machines, reading inventory, seeing who rolled back) is read.
**A compromised console therefore cannot change a fleet's policy**; it can only
see. Design for that split on purpose: the usual arrangement, one admin role
that can do both, makes the console the whole security boundary.

### What the server may see, which bounds this more than what it may do

Inventory rows are the machine's claim about itself, and the server keeps them.
Retention is the operator's decision, and this design does not get to make it.
The *machine's* half is not negotiable, and a test can hold it: the client must
be able to print exactly what it would send without sending it, the way `rime
channel report` does today. `rime fleet report --dry-run` is that verb, and the
suite that proves it asserts an absence, the pattern
`tests/test-rime-channel.sh` already uses.

### What the server must not be able to do

The machine-side list has a mirror, and the protocol's structure enforces most
of it, which is the argument for the transport above. With a poll of signed
documents, the server **cannot** reach a machine between polls, run anything,
approve a privilege request, enrol a machine that did not run the verb, or stop
one leaving. No rule forbids these; the protocol has no message that would do
any of them.

## What must never be built

Each item is something a fleet product normally ships, and each would break
something Rime already guarantees.

1. **A remote root shell, or any remote exec.** `rime-agentd` is unprivileged
   and per-user, and `AGENTS.md` requires it to stay that way: no polkit
   action, no system-bus name, no setuid helper. A fleet channel with exec is
   that helper by another name.
2. **A fleet that can approve a privilege request.** §7's origins exist so a
   request from somewhere other than a human at this machine is
   *distinguishable*. A fleet approving on the user's behalf erases the
   distinction the whole model is built on.
3. **Silent enrolment.** No enrolment through an image build, a blueprint, a
   plugin, or an MCP server. One verb, run by somebody who can answer for it.
4. **Telemetry from unenrolled machines.** Including "anonymous" counts. The
   sentence in `docs/update-channels.md` is the contract.
5. **Fleet-owned user accounts.** `rime user` owns account creation
   (`docs/multi-user.md`), and a second write path into account state is the
   defect class P2-016 spent a round on.

## What this design does not settle

One gap at a time, because a design document that ends with a summary instead
of a gap list gets built wrong.

* ~~**The transport.**~~ **Settled above: the client polls, `relay/` is not in
  the fleet path, and a response is a signed document, never a command.** What
  remains open is narrower, and is engineering, not design: the interval, and
  whether the freshness bound is an expiry in the document or a maximum age the
  client enforces. The rollout document answered the second question for itself
  with both: it carries its own `expires`, and the client caps any document's
  age at 30 days (`MAX_DOCUMENT_AGE` in `rimed_core::channel`). Neither
  question changes the threat model; the direction of the connection was the
  part that did.
* ~~**Multi-tenancy on the operator side.**~~ **Settled above: tenancy is key
  separation, read is a server permission and write is a cryptographic one.**
  The delegation format remains open (whether the chain from the root key
  reuses `rime trust`'s cosign machinery verbatim or only its shape), and it
  cannot be settled without a server to try it against.
* **`relay/` is not in the fleet path.** It is deployed and serving the
  phone-to-desktop pairing it was written for (its README records that
  `src/index.js` is live), and the transport decision above keeps it out of the
  fleet.
* ~~**What a ring ceiling costs to serve.**~~ **Worked out, and the cost that
  matters is elsewhere.** `docs/update-cost.md` records that a `core` rebuild costs each
  machine about 5 GB, and a ramp does not change that: the same machines pull
  the same layers, later. A ramp changes the *shape* of the pull: a 5% ring
  spreads the same 5 GB over the days the ramp takes instead of the hours a tag
  move takes, which is a saving for the site and nothing for the registry.
  Serving the ceiling itself is negligible, written down here so nobody
  re-derives it: the rollout document is a few hundred bytes, fetched once per
  `rime update` alongside a manifest the machine was fetching anyway, four or
  five orders of magnitude under the image pull it gates. **The real cost runs
  the other way:** a ramp makes a bad build take *longer* to reach everybody,
  and therefore longer to be noticed, so the fleet's own reporting has to be at
  least as fast as the ramp or the ramp buys nothing. That constrains the
  reporting interval, which is not settled because the interval is not.
* ~~**Whether the fleet-assigned machine id can be rotated.**~~ **Settled, by
  separating two identifiers that get conflated.** Only one of them is the
  fleet's:
  * The **rollout slot** is 0–99, derived locally from `/etc/machine-id`, never
    transmitted, and belongs to the machine. Regenerating `/etc/machine-id`
    reassigns it: a machine mid-ramp jumps into or out of the current cohort,
    once, and nothing else breaks. That is acceptable, and stated here because
    it surprises people: the slot is not stable across a `systemd-firstboot
    --setup-machine-id`, a cloned disk, or a re-install.
  * The **fleet identity** is the enrolment record's key, and it rotates the way
    every other long-lived key in this system rotates: by re-enrolling, which is
    an act at the keyboard. It is NOT derived from `/etc/machine-id`, on
    purpose: an identity a fleet assigns must not change because a user reset
    an unrelated file, and the fleet cannot revoke an identity the machine
    derives.
  * If they were the same value, rotating the identity would re-roll the
    machine's rollout slot, so revoking a device's fleet access would move it in
    the ramp: two unrelated operations wired together, and nobody would find
    that by reading either one.
* **The evidence standard for compliance.** Still open, for the same reason as
  before. A row from `rime doctor --json` is a claim by the machine about
  itself; a fleet that treats it as proof has outsourced its trust to the
  device it is checking. Attestation is the answer, and it depends on L-001's
  TPM work, which is `partial`. This design can already say which rows would
  become attestable and which never will: Secure Boot state, the booted digest
  and the measured boot path are TPM-quotable; "is the disk encrypted" is
  quotable through the PCR policy that unsealed it; "is the firewall on", "is
  this app installed" and every user-space row are not. A fleet that reports
  those as verified is reporting a self-assessment with a certificate stapled
  to it.
* **Nothing here has been deployed or written.** The decisions above come with
  their reasoning, which is what a design item delivers; they are not a client
  or a server. Two pieces of code exist. The rollout document left this design
  because it is not part of a fleet: it ships, for everybody, and
  `docs/update-channels.md` describes it. `RolloutQuery::fleet_ceiling` is the
  parameter a fleet client would set, and every machine passes `None`.
