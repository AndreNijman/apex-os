# APEX Fleet — a design, and nothing else yet

Roadmap P2-019. This document is the deliverable: an architecture for managing
many APEX machines that scales down to one, written against what the system
already does rather than against the shape a fleet product usually takes.

**No code here exists.** There is no `apex fleet` verb, no enrolment record, no
server. Every mechanism named below either already ships and is cited, or is
marked as not built. The distinction is kept in every section, because a design
document that reads as a feature list is how a roadmap item gets recorded as
done twice.

One thing that used to be in this document has left it, and that is the most
useful thing to know before reading the rest. The staged-rollout pointer — the
percentage that has to change between builds — was going to be a fleet endpoint,
because an OCI label is baked once and a fleet is the obvious thing that could
serve a number. It is now a signed document in the registry that **every**
machine reads, enrolled or not, and it ships. Designing it as a fleet feature
would have made a personal machine's update path worse in order to give a
managed one something, which is the failure mode the rule below exists to
prevent, arrived at by accident rather than on purpose.

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
| staged rollout | a stable slot 0–99 per machine, and a signed rollout document in the registry that says how far the ramp has got | `apex channel rollout`, P1-046 |
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

## The transport

**Not built, and now decided.** The client **polls**, over ordinary HTTPS, and
`relay/` is not in the fleet path at all.

The decision falls out of the action list rather than out of a preference for
polling. Read *What must never be built* first: no remote exec, no fleet-side
approval, no silent enrolment. What is left that a fleet may actually do to a
machine is set its channel, set a ring ceiling, and hold its updates — and the
last of those is the only one anybody wants in a hurry. None of them needs
sub-minute latency. **A transport chosen for push latency would buy speed for
actions this design does not permit**, and would charge for it twice:

* A machine that holds a connection is a machine that is continuously
  reachable. An unenrolled machine has no fleet code path to disable *because
  there is no socket*, and that is the rule that comes before the architecture
  being kept by the shape of the thing rather than by a flag somebody can flip.
* A relay that carries fleet traffic learns which machines are awake, and when.
  That is a continuous liveness map — exactly the class of data
  `docs/update-channels.md` promises not to collect, arrived at by a side door.
  `relay/` was written for a phone talking to its owner's desktop, where both
  ends belong to the same person; a fleet relay is a third party watching, and
  "it only sees ciphertext" is not an answer to that.

So the machine decides when it talks, and nothing reaches it in between.

### What a poll is

The same shape as `apex channel report`, which already composes a payload the
machine sends and can print instead. Out goes the inventory row; back comes a
**document, never a command**: channel, ring ceiling, a hold flag, a policy
body. The client decides what to do with it, and everything it may do is
something a local verb already does. A response that named a command to run
would be remote exec through a data channel, which is item 1 of the list this
design refuses, wearing a different hat.

### When a poll happens

On a jittered interval derived from the **stable slot 0–99** that `apex channel
status` already computes from `/etc/machine-id`. That is reuse rather than
coincidence: staging a rollout and staggering a poll both need a stable
per-machine number in the same range that nobody has to configure, and a fleet
of ten thousand machines polling on the hour is a self-inflicted outage.

The cost is stated rather than hidden: **"hold this machine's updates" takes
effect at the next poll.** An operator gets no answer faster than the interval
and no answer at all from a machine that is off. Both are acceptable for the
three actions above; neither would be if remote exec were on the list, which is
another way of saying the transport and the permission list have to be chosen
together.

### What makes a response trustworthy

Not the transport. The poll is authenticated with the machine's long-lived key
— the identity enrolment already used, not a second one — and the **response is
signed by the operator's key recorded at enrolment**. A hostile network, a
compromised CDN or a misissued certificate can then deny service and cannot
change a machine's channel, which is the property worth having: the signature
is load-bearing and TLS is convenience.

Two things a signature alone does not give, both of which have to be in the
document rather than around it:

* **Replay.** An old, validly signed "you are on `edge`" is a downgrade attack
  unless the document carries the machine id and a monotonic counter the client
  refuses to go backwards on.
* **Expiry.** A document with no freshness bound means a fleet that stops
  answering silently pins every machine to its last instruction. It should
  expire, and an expired document should leave the machine on its own local
  configuration rather than on the fleet's last word.

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

The pointer did not ship when this section was first written, and
`docs/update-channels.md` said why:

> nothing publishes a percentage yet. The gate reads it from an image label and
> treats an image without one as reaching everybody… A ramp — 1%, then 5%, then
> 25% — needs the number to change between builds, and a label is baked once.

**That quotation has since been superseded, and the way it was resolved matters
more than the resolution.** The obvious conclusion from "a label is baked once" was
"so a staged rollout needs a fleet endpoint", and that conclusion would have
made the ramp a feature only managed machines get — which is precisely the
shape the rule at the top of this document exists to refuse.

The pointer went somewhere else: a **signed rollout document**, published at
`…:rollout` in the same registry as the image and re-published whenever the
number moves. It is mutable, it needs no server, and a personal machine reads it
with the cosign verification it already performs on the image. It ships;
`docs/update-channels.md` describes it and `apexd_core::channel::decide_rollout`
is the whole of what a machine does with one.

So the ring is:

```
ring := { channel, max_slot, halt }
```

— and **the fleet's half of it is now only the ceiling**, because the other two
are served to everybody:

| part | who serves it | to whom |
| --- | --- | --- |
| channel | the tag in the machine's bootc origin | everybody; `apex channel set` |
| the ramp | the signed rollout document | everybody, per channel |
| `halt` | the same document | everybody, per channel |
| `max_slot` | a fleet | its enrolled machines only |

What a fleet adds is therefore narrow and worth stating exactly: **it can hold
its own machines further back than the publisher's ramp, and it can never push
them ahead of it.** That is not a promise in this document — it is a parameter
of the decision function, `RolloutQuery::fleet_ceiling`, which only ever lowers
the percentage, with a test that fails if a ceiling above the ramp raises it.
Every machine passes `None` today, because no fleet client exists.

`halt` moved out of the fleet's hands for the same reason the ramp did, and
there are now three stops rather than one. The per-machine stop that already
ships refuses an update when *this* machine came back unhealthy from the last
one. The publisher's halt, in the rollout document, stops a digest reaching
anybody. The third — the one that fires when enough machines in a cohort came
back unhealthy — is the one this design does not have, and its absence is not a
transport problem: counting a cohort needs machines to report health to their
fleet, which is the section below, which is opt-in, and which nothing receives.
So the halt that ships is a person's judgement, dispatched after reading
evidence that came from somewhere else. The automatic one stays on the not-built
list rather than being written up as though the wiring were all that was
missing.

Whichever of the three fires, the machine's own stop stays authoritative for the
machine: a fleet may hold a rollout, and may not force one past a local refusal.

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

## The server side

**Not built, and deliberately not part of APEX.** APEX ships a client and a
protocol. A fleet that only worked against one hosted server would make
"optional" untrue for anybody who cannot or will not use it, so the server is a
separate deliverable and this section is what it has to be, not what it is.

### The smallest correct server is a signing tool and a bucket

Because a poll response is a signed document and nothing else, the minimum
viable fleet server has **no always-on service and no database**: one signed
document per machine, served as a static object. An operator with fifty
machines can run a fleet out of object storage and a script. That is not a
toy — it is the same artefact the large version serves, and it means the
promise that personal installs are not managed devices costs an operator
nothing to keep.

Where that stops is **group membership**: deciding which machine gets which
document is the part that wants a database once the answer is not "one file per
machine id". A few thousand machines is the honest edge of the static version.

### Tenancy is key separation, not rows

**The unit of authority is a signing key, not a login.** A fleet *is* a key
pair; a machine is enrolled to a public key; two fleets are two keys. A server
that separated tenants only by partitioning rows would have one place at which
every tenant's policy could be rewritten — and since the client checks a
signature, that server could not actually do it, which is the point. Tenancy
that is cryptographic rather than administrative fails safe.

What the operator side still has to support, and what that implies for the
machine:

* **More than one human, and a record of who signed what.** So the machine
  records the fleet's **root** key at enrolment and accepts a document signed by
  a delegated key whose delegation chains to that root — the shape `apex trust`
  and P1-047's cosign verification already use, rather than a second one.
* **Revoking a signer without re-enrolling every machine.** Which is the same
  requirement stated from the other end, and the reason the root key is what
  enrolment pins.

### Read is a server permission; write is a cryptographic one

The only server-side action that changes a machine is moving it between
documents, and that needs the signing key. Everything else an operator console
does — listing machines, reading inventory, seeing who rolled back — is read.
So **a compromised console cannot change a fleet's policy**; it can only see.
That split is worth designing for explicitly, because the usual arrangement
(one admin role that can do both) makes the console the whole security boundary.

### What the server may see, which bounds this more than what it may do

Inventory rows are the machine's claim about itself, and the server keeps them.
Retention is the operator's decision and this design does not get to make it —
but the *machine's* half is not negotiable and is test-shaped: the client must
be able to print exactly what it would send without sending it, the way `apex
channel report` does today. `apex fleet report --dry-run` is that verb, and the
suite that proves it is the one that asserts an absence — the pattern
`tests/test-apex-channel.sh` already uses.

### What the server must not be able to do

The machine-side list has a mirror, and most of it is structural rather than
enforced, which is the argument for the transport above. A poll of signed
documents means the server **cannot** reach a machine between polls, cannot run
anything, cannot approve a privilege request, cannot enrol a machine that did
not run the verb, and cannot stop one leaving — not because it is forbidden to,
but because there is no message in this protocol that would do it.

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

* ~~**The transport.**~~ **Settled above: the client polls, `relay/` is not in
  the fleet path, and a response is a signed document rather than a command.**
  What is still open is narrower and is engineering rather than design — the
  interval, and whether the freshness bound is an expiry in the document or a
  maximum age the client enforces. Neither changes the threat model; the
  direction of the connection was the part that did.
* ~~**Multi-tenancy on the operator side.**~~ **Settled above: tenancy is key
  separation, read is a server permission and write is a cryptographic one.**
  What is still open is the delegation format — whether the chain from the root
  key reuses `apex trust`'s cosign machinery verbatim or only its shape — and
  that cannot be settled without a server to try it against.
* **`relay/` is not in the fleet path**, and its own README says
  `src/index.js` has never been executed.
* ~~**What a ring ceiling costs to serve.**~~ **Worked out, and it is not the
  number anybody expected to be looking at.** `docs/update-cost.md` records that
  a `core` rebuild costs each machine about 5 GB, and a ramp does not change
  that: the same machines pull the same layers, later. What a ramp changes is
  the *shape* of the pull — a 5% ring spreads the same 5 GB over the days the
  ramp takes rather than the hours a tag move takes, which is a saving for the
  site and nothing at all for the registry. Serving the ceiling itself is
  negligible in a way worth writing down so nobody re-derives it: the rollout
  document is a few hundred bytes, fetched once per `apex update` alongside a
  manifest the machine was fetching anyway — four or five orders of magnitude
  under the image pull it gates. **The cost that is real is the opposite one:**
  a ramp makes a bad build take *longer* to reach everybody and therefore longer
  to be noticed, so the fleet's own reporting has to be at least as fast as the
  ramp or the ramp is buying nothing. That is a constraint on the reporting
  interval, and it is not settled because the interval is not.
* ~~**Whether the fleet-assigned machine id can be rotated.**~~ **Settled, by
  separating two things that get conflated.** There are two identifiers and only
  one of them is the fleet's:
  * The **rollout slot** is 0–99, derived locally from `/etc/machine-id`, never
    transmitted, and belongs to the machine. Regenerating `/etc/machine-id`
    reassigns it — a machine mid-ramp jumps into or out of the current cohort,
    once, and nothing else breaks. That is acceptable and is stated here because
    it is surprising: the slot is not stable across a `systemd-firstboot
    --setup-machine-id`, a cloned disk, or a re-install.
  * The **fleet identity** is the enrolment record's key, and it rotates the way
    every other long-lived key in this system rotates: by re-enrolling, which is
    an act at the keyboard. It is deliberately NOT derived from
    `/etc/machine-id`, because an identity a fleet assigns must not change
    because a user reset an unrelated file, and an identity the machine derives
    cannot be revoked by the fleet.
  * What breaks if they are the same value: rotating the identity would silently
    re-roll the machine's rollout slot, so revoking a device's fleet access
    would move it in the ramp — two unrelated operations wired together, and
    nobody would find that by reading either one.
* **The evidence standard for compliance.** Still open, and the reason has not
  changed. A row from `apex doctor --json` is a claim by the machine about
  itself; a fleet that treats it as proof has outsourced its trust to the device
  it is checking. Attestation is the real answer and depends on L-001's TPM
  work, which is `partial`. What *can* be said now is which rows would become
  attestable and which never will: Secure Boot state, the booted digest and the
  measured boot path are TPM-quotable; "is the disk encrypted" is quotable
  through the PCR policy that unsealed it; "is the firewall on", "is this app
  installed" and every user-space row are not, and a fleet that reports them as
  verified is reporting a self-assessment with a certificate stapled to it.
* **Nothing here has been deployed or written.** The decisions above are
  decisions with their reasoning, which is what a design item delivers; they are
  not a client, a server, or a line of code. The one exception is the rollout
  document, which is no longer part of this design because it is not part of a
  fleet: it ships, for everybody, and is described in
  `docs/update-channels.md`.
