# APEX Fleet — a design, and nothing else yet

Roadmap P2-019. This document is the deliverable: an architecture for managing
many APEX machines that scales down to one, written against what the system
already does rather than against the shape a fleet product usually takes.

**No code here exists.** There is no `apex fleet` verb, no enrolment record, no
server. Every mechanism named below either already ships and is cited, or is
marked as not built. The distinction is kept in every section, because a design
document that reads as a feature list is how a roadmap item gets recorded as
done twice.

## The rule that comes before the architecture

> Enterprise/fleet support remains optional and does not turn personal APEX
> installs into managed devices by default.

That is the third acceptance criterion, and putting it first is deliberate: it
is a constraint on the shape of everything else, not a setting.

The mechanism is that **an unenrolled machine has no fleet code path to
disable**. A fleet is a thing a machine joins by an explicit, authenticated act
that writes one record; with no record, the client is inert — it starts no
timer, opens no socket, and contacts nothing. "Inert" has to be measurable, and
the way this repository measures that kind of claim is a test that asserts the
absence: `tests/test-apex-channel.sh` already proves `apex channel report` sends
nothing by printing the payload and asserting nothing left the machine, and
`docs/update-channels.md` says so in the plainest sentence in the tree —
*"Nothing, to nobody. APEX operates no telemetry service."* A fleet client must
be able to keep that sentence true for an unenrolled machine, and the test must
be the kind that fails if it stops being true.

The second half of the rule is that **unenrolling works from the machine**. A
device whose owner cannot leave a fleet from the keyboard is not a personal
computer that opted in; it is a managed device that was sold as one. A fleet
may refuse to *re-enrol* a machine that left, and it may report that it left.
It may not hold it.

## What already exists, per machine

Nothing below is proposed. It is the substrate, and the design's main claim is
that a fleet is a distribution problem on top of it rather than a new agent.

| need | what ships today | where |
| --- | --- | --- |
| declarative state | `apex blueprint diff/apply`, idempotent, secrets never in the bundle | `apexd-core/src/blueprint.rs`, BASE-007 |
| update rings | `edge`/`beta`/`candidate`/`stable`, promotion gated in CI on a cosign signature | `apex channel`, P1-046 |
| staged rollout | a stable slot 0–99 per machine, derived from `/etc/machine-id` | `apex channel status`, P1-046 |
| a health verdict | four rows that count, measured by the same probes as recovery | `ops::update`'s rollout stop |
| inventory facts | channel, tag, digest, healthy, reasons | `apex channel report` |
| device identity | a long-lived key, and paired devices | `apex-remote-core::identity`, `apex remote` |
| credentials | root-owned per-uid store, no verb returns a value | `apex-secretd`, P0-002 |
| capability grants | per project, per operation, spendable by the daemon only | P1-001 |
| supply chain | SBOM, provenance, cosign verification | `apex provenance`, `apex trust`, P1-047 |
| recovery | previous deployment, rollback, doctor, Safe Graphics | `apex recover`, `docs/recovery.md`, P2-018 |
| privilege | seven request origins, `cloud-job` among them, declared not observed | `apex-agent-core::origin`, §7 |

The last row is the one a fleet design most often gets wrong: APEX already has a vocabulary for "this request came from
somewhere other than a human at this keyboard", and `cloud-job` is one of its
values. A fleet is a *remote origin*. It does not need a new privilege model; it
needs to be honest about which origin it is.

## Enrolment

**Not built.** The shape:

```
apex fleet status                 # unenrolled, and what that means
sudo apex fleet join <token>      # the one act that changes anything
sudo apex fleet leave             # from this keyboard, always
```

A join token carries the fleet's identity and an endpoint. It is single-use and
short-lived. The machine already has a long-lived key pair
(`apex-remote-core::identity::Identity::load_or_create`), and the enrolment
exchange should be the pairing exchange APEX already performs for a phone — the
same Noise handshake, the same store, a different peer role. Inventing a second
device-identity mechanism would mean a second place for a key to be wrong.

Three properties the exchange has to have, each one something this repository
learned the hard way:

* **The token proves the operator, not just the endpoint.** A token that only
  says where to connect is a token an attacker can mint by standing up a server.
* **Enrolment is refused from inside a managed agent session.** `apex-secretd`
  already refuses `Add`, `Remove`, `Grant` and `Approve` from a caller whose
  cgroup or process ancestry says it is inside `apex-agentd`
  (`apex-secretd/src/main.rs::refuse_a_session`). Joining a fleet is a larger
  act than granting a capability and gets at least the same gate.
* **Every verb is checked, not just the interesting one.** P2-016 found
  `apex-remoted` checking the caller's identity for `Pair` and for nothing else,
  so a second local account read the owner's machine key out of `Status`, the
  paired-device list out of `Devices`, and reached the device store through
  `Revoke`. A fleet client has more verbs than that and the same failure mode.
  Authorisation belongs in a `match` over every request variant with a
  table-driven test that names each one — `apex-remoted/src/control.rs::
  authorized` is the pattern.

What enrolment writes is one record under `/var/lib/apex-fleet/`, root-owned,
`0700`: the fleet id, the endpoint, the operator's public key, and the time. The
presence of that file is the only thing that makes any of the rest run.

## Inventory

**Partly built.** `apex channel report` already computes the payload and already
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

**The machine id sent is not `/etc/machine-id`.** The rollout slot is derived
from it and `apex channel status` already says the slot "is derived from this
machine's id and is never sent anywhere". A fleet needs a stable handle; it does
not need that one, and sending it would turn a value the machine keeps to itself
into a fleet-wide correlator.

**There is no user in it.** Not the account names, not the home directory sizes,
not what was installed by whom. An operator managing a device is not thereby
managing the person using it, and a report that carries the person is the report
that gets a fleet product banned from a school. If a future criterion needs
per-user facts, they get their own consent surface and their own section.

## Compliance

**Partly built. This section has the least new work in it.**

APEX already computes, on the machine, everything a compliance check would ask
for, in a form with stable identifiers:

* `apex recover status --json` — eight rows with stable ids
  (`current-deployment`, `previous-deployment`, `secure-boot`, `filesystem`,
  `gpu-driver`, `apex-shell`, `network`, `package-extensions`). The ids are
  already a compatibility surface, keyed on by apex-shell's `RecoveryService`.
* `apex doctor --json` — the full health report.
* `apex trust` and `apex provenance` — signature and SBOM state.
* `apex channel status` — which ring, and whether this machine is behind it.

A compliance policy is therefore a **predicate over facts that already have
names**, not a new agent that re-measures the machine. That matters beyond
tidiness: a second measurement path would eventually disagree with the first,
and the machine's own report is the one the user can run.

The one thing a policy must never be is a *remediation with root*. A fleet may
say a machine is out of compliance and may refuse it a resource; it may not
silently repair it. Repair is `sudo apex …` run by somebody who can answer for
it, and APEX's polkit actions are `auth_admin` with `allow_inactive=no`
precisely so that somebody has to be at the keyboard
(`docs/multi-user.md`).

## Update rings

**Mostly built, and the missing piece is already written down.**

A ring is a pair: a channel, and a ceiling on the rollout slot. The channel half
ships (`apex channel set`, promotion gated in CI on a cosign signature from this
repository's `main` workflow). The slot half ships (`0–99`, stable across
reboots, derived from `/etc/machine-id`).

What does not ship is the pointer. `docs/update-channels.md` says it plainly:

> nothing publishes a percentage yet. The gate reads it from an image label and
> treats an image without one as reaching everybody… A ramp — 1%, then 5%, then
> 25% — needs the number to change between builds, and a label is baked once.

**A fleet is the natural home for that pointer**, and that is the single most
useful thing this design contributes. An OCI label is baked once per build; a
fleet endpoint can serve a number that changes. So:

```
ring := { channel, max_slot, min_digest_age, halt }
```

An enrolled machine asks its fleet for its ring's current ceiling before
updating, and compares it against the slot it already computes locally. An
unenrolled machine keeps today's behaviour exactly — no label, reaches
everybody — which is what makes this an addition rather than a change.

`halt` is the fleet-wide form of the rollout stop that already exists per
machine. The per-machine stop refuses an update when *this* machine came back
unhealthy from the last one; the fleet-wide one refuses when enough machines
did. Same verdict vocabulary, counted across a cohort. The threshold is a
number an operator sets and an operator can be wrong about, so the machine's own
stop stays authoritative for the machine: a fleet may halt a rollout, and may
not force one past a local refusal.

## Managed secrets and certificates

**Not built, and the hardest section.**

`apex-secretd`'s store is **per uid** (`/var/lib/apex-secretd/users/<uid>/`), and
that is load-bearing: authorisation is `SO_PEERCRED`'s uid and there is no verb
that names another account. A fleet secret does not belong to a uid. It belongs
to the machine.

The design is a second namespace under the same daemon, not a second daemon:

* `/var/lib/apex-secretd/machine/` — root-owned, `0700`, alongside `users/`.
* A machine credential is **readable by no local account at all.** The daemon
  spends it, exactly as it spends a user's, and the same compile-time property
  holds — `SecretValue` has no `Serialize` impl and no `Response` variant can
  carry a credential.
* What a local caller may ask for is a *capability*, and which capabilities a
  fleet credential backs is part of the enrolment record, not of a user's grant
  table.

Certificates are the easier half and should not be folded into the harder one. A
fleet's CA bundle and its 802.1X client certificates are files with owners and
expiry dates, not brokered operations. They belong in the image's trust
configuration with a fleet-managed drop-in, and the property to preserve is that
a machine which leaves a fleet stops trusting the fleet's anchors — which means
the anchors go in a directory `apex fleet leave` empties, not in the base trust
store.

This is not a theoretical need. An 802.1X profile that validates no CA
certificate authenticates the client to the network and the network to nobody,
and profiles in that state are ordinary on machines joined to a school or
office network by hand. A fleet that distributed an anchor and pinned it would
be closing a real hole rather than adding a feature.

## App policy

**Partly built.** The pieces:

* `apex blueprint` describes desired apps and applies idempotently, and its sync
  bundle provably carries no secrets (BASE-007's evidence: a sentinel token
  planted in the old broker's store, absent from the exported bundle).
* `apex-perm-core` reports what an application actually holds — portals,
  Flatpak context, device ACLs — without inventing a second enforcement layer
  (P1-061).
* Plugin trust decides whether third-party code may run at all (P1-025).

A fleet app policy is a blueprint the machine did not write, plus a deny list,
plus the honesty that `apex-perm-core` already insists on: **report what is
true, do not claim what is not enforced.** A policy that says "the camera is
disabled" when the mechanism is a Flatpak override a user can remove is a
policy that lies to an operator. The existing crate's rule — it is the
truth-teller, not a new enforcement layer — is the right rule for the fleet
layer too.

## Remote health and recovery

**Partly built.** A machine that is unwell is the machine least able to tell
anybody, which is why the recovery work sits under P2-018 and this section is
short.

* The per-machine half exists: `apex recover status`, `apex doctor`,
  `sudo apex rollback`, and APEX Safe Graphics, which comes up on the CPU with
  its own compositor configuration when the normal desktop does not paint.
* The fleet half is *reporting* that state, not driving it. An operator can see
  that a machine rolled back and why. An operator cannot roll a machine back
  remotely in this design, and the reason is the one stated under Compliance: a
  deployment change is `auth_admin`, `allow_inactive=no`, answered by somebody
  at the keyboard.

The one remote action worth designing is the narrow one: **hold this machine's
updates.** It is reversible, it cannot brick anything, and it is what an
operator actually wants in the hour after a bad release.

## What must never be built

A list, because each item is something a fleet product normally ships and each
would break something APEX already guarantees.

1. **A remote root shell, or any remote exec.** `apex-agentd` is unprivileged
   and per-user and `AGENTS.md` requires it stay that way — no polkit action, no
   system-bus name, no setuid helper. A fleet channel with exec is that helper
   by another name.
2. **A fleet that can approve a privilege request.** §7's origins exist so a
   request from somewhere other than a human at this machine is
   *distinguishable*. A fleet approving on the user's behalf erases the
   distinction the whole model is built on.
3. **Silent enrolment.** No enrolment through an image build, a blueprint, a
   plugin, or an MCP server. One verb, run by somebody who can answer for it.
4. **Telemetry from unenrolled machines.** Including "anonymous" counts. The
   sentence in `docs/update-channels.md` is the contract.
5. **Fleet-owned user accounts.** `apex user` owns account creation
   (`docs/multi-user.md`), and a second write path into account state is the
   defect class P2-016 spent a round on.

## What this design does not settle

Named one at a time. A design document that ends with a summary instead of a gap
list is the one that gets built wrong.

* **The transport.** Pairing exists, and `relay/` is an untrusted Noise_IK
  rendezvous that has never been deployed. Whether a fleet client polls, holds a
  connection, or is pushed to through the relay is undecided, and the answer
  changes the threat model.
* **Multi-tenancy on the operator side.** Everything above is written from the
  machine's point of view. There is no design here for the server, and the
  server is where an operator's own access control lives.
* **What a ring ceiling costs to serve.** `docs/update-cost.md` records that
  core rebuilds cost the fleet about 5 GB each. A ramp changes when machines
  pull, not how much, but the interaction has not been worked out.
* **Whether the fleet-assigned machine id can be rotated**, and what breaks when
  it is.
* **The evidence standard for compliance.** A row from `apex doctor --json` is a
  claim by the machine about itself. A fleet that treats it as proof has
  outsourced its trust to the device it is checking. Attestation is the real
  answer and it depends on L-001's TPM work, which is `partial`.
