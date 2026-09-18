# Can a browser capsule log in? — the decision, and what it costs

P2-012's acceptance line is "isolated browser profile/cookies/downloads and
capability auth". Three of those four are built and measured
(`docs/browser-capsule.md`). The fourth is not: **a capsule cannot present a
credential to a site**, and that is why the item is recorded `partial`.

This page is the written decision the gap needs before anybody writes code for
it. It says what the framework guarantees today, what each route would
change, which measurement kills or permits each one, and what the chosen route
would cost.

**None of the four routes is built**, and the reason is at the bottom of this
page: route B is blocked on a question nobody has answered, and the other
three are rejected or need a package. One thing on this page IS built, and it
is not a route — the **per-run CA bind** (`--trust-ca`, protocol 10), which
lets a capsule automate an intranet site behind a private root. It closes a
different gap with none of route B's machinery; see the last section.

## What is decided

**The route is B — the daemon presents the credential and the capsule never
holds it.** Route A, handing the capsule a session, is rejected, and the reason
is a measurement rather than a preference: a value inside a capsule reaches the
caller through the nomination boundary, which is a path the capsule's own
design puts there on purpose.

Neither is built. Route B needs two changes to the agent protocol and a TLS
server in `apex-agentd`, both scoped below. It also changes a sentence
`docs/browser-capsule.md` states as a property — "a tunnel is opaque" — which
is the sort of thing that should be seen before it is built rather than found
in a diff.

**And "the route is B" is a narrower statement than it reads as.** B is the
chosen *shape* — of the four sketched here it is the only one that authenticates
a capsule to an arbitrary header-auth site without the capsule ever holding the
value. It is not a decision to build it, because building it means the daemon
reads the plaintext of one connection, and whether that is acceptable is a
product question nobody has answered. It is stated as a question with its costs
at the bottom of this page. Until it has an answer, P2-012 stays `partial`, and
this page is the reason rather than an excuse.

## What a capsule can and cannot be handed today, measured

Two facts decide the shape of every route, and both were measured in
`tests/browserlab/run-browserlab`'s `authentication` flow rather than reasoned
about.

**An engine cannot put anything in a capsule except a file it writes.** The
first attempt at a control exported a variable beside `apex browser` and
reached nothing at all: a session's environment is built by the daemon, and the
sandbox does not inherit the caller's. So the control was rerouted through the
capsule *profile*, which is a file the engine does write — and that is also the
only route a credential could take if the engine were the one carrying it.

**Anything a capsule can see, the caller can take home.** The same flow's second
control writes the value into a file inside the capsule and nominates it: it
lands on the host. That is not a defect in the nomination loop — carrying files
out is what `--download` is for — but it is the fact that decides between the
two routes.

Six negative observations hold today, each with that pair of controls behind
them: the credential's value is not in the capsule directory, not in the
profile, not in the capsule's home, not in the session's environment, not on
the browser's command line, and nothing is harvested for a caller to collect.

## Route A — hand the capsule a session

A provider performs a login against the pinned host, gets a session cookie
back, and the cookie is written into the capsule's fresh profile before the
browser starts. The capsule holds a short-lived credential rather than the
stored one.

### It works, and that had to be measured

A cookie written into a fresh profile's `cookies.sqlite` **is** sent by the
browser. Measured on Firefox 155 with four profiles: the row updated in place
in a profile the browser built, the row deleted and reinserted, the database
dropped alone into an otherwise empty profile directory, and an untouched
control. All four behaved as the mechanism requires; the third is the one that
matters, because it is the shape a capsule has.

`sqlite3` is not in the image and `python3` is, with its `sqlite3` module, so
the writing half needs no package.

### The trap in it, which is this repository's most familiar shape

`moz_cookies.expiry` is **milliseconds** in schema version 17, not the seconds
every older reference gives. A cookie seeded with a seconds-valued expiry is
read as long expired, is silently not sent, and the run still exits 0 having
rendered the page and written the screenshot. The first probe did that and
reported "a pre-seeded cookie is not sent" — a wrong conclusion about the
whole route, from a unit error, with no error anywhere.

Anything that writes this file has to take the schema from a database the
browser itself created, the way the probe did, rather than from a constant
somebody typed.

### Why it is rejected

The framework's rule is that an agent uses a credential without holding it —
`apex secret list` never prints a value and `apex browser` never asks for one.
Route A breaks that in two places at once:

* **The caller gets the value.** The capsule holds it, `--download` carries
  files out of capsules, and the `authentication` flow demonstrates the whole
  path with a marker string. `HttpOnly` narrows the JavaScript route to the
  cookie and does nothing about the process route: the capsule's own browser
  has the jar on disk.
* **It needs a provider per site.** "Log in" is not one operation. A provider
  that knows one site's login form is a provider that knows one site, and
  §13.2's vocabulary is declared per provider — so the work does not compose,
  and every new site is another `ProviderSpec`.

A short lease and the destination pin bound the damage — the capsule can only
reach the host the credential is for, because the same capability sets its
allowlist, and since protocol 9 that pin is enforced by the egress proxy rather
than checked by the engine — but they bound it *after* accepting that the caller ends up
holding a credential they were never supposed to see. That is a different
product from the one the framework describes, and it should not arrive as a
flag.

If it is ever wanted anyway, the honest form is the one that keeps the value
out of user processes: **`apex-secretd`, which is root, writes the cookie file
into the capsule directory itself**, and the engine never sees it. An engine
that read the value to write `cookies.sqlite` would have moved it through an
unprivileged process, which is the thing the store exists to stop.

## Route B — the daemon presents the credential

`apex-agentd` already stands between a capsule and every host it reaches. For a
destination bound by `--capability`, and only for that one, the daemon
terminates TLS with a certificate it mints for the run, adds the credential's
header, and originates its own TLS connection to the site. The capsule trusts a
CA that exists for the length of one capsule and is trusted by nothing else on
the machine.

The capsule never holds the value, so all six negative observations above keep
holding, and nothing a caller nominates can contain a credential that was never
in the capsule.

It is also general in the way Route A is not: it works for every site whose
authentication is a header, with no per-site provider. `ServiceInfo::auth`
already distinguishes `bearer` from `raw` for this reason.

### The prerequisite that looked like a blocker, and is not

A fresh profile trusts the system CA store and nothing else, and `nss-tools` is
not in the image — so `certutil` cannot add one. That was recorded as closing
the question. It does not:

**A `policies.json` bound inside the capsule's namespace installs a CA with no
`certutil` at all.** Measured, with a control: a fresh profile refused a
self-signed loopback server and sat on the refusal until it was killed; the
same profile, the same server, with
`{"policies":{"Certificates":{"Install":["…/ca.pem"]}}}` bound over
`/etc/firefox/policies/policies.json` inside the namespace only, completed the
handshake and rendered the page. The machine's own file was untouched
throughout and compared clean afterwards.

That made gap 5 on this unit's card — "a CA a capsule could be told to trust" —
a per-session `--ro-bind` in `apex_agent_core::sandbox`, one field on the
session request rather than a package that is not in the image. **It is built**
(`--trust-ca`, protocol 10); the bottom of this page records what it cost and
what the build found that this paragraph did not foresee.

### The join, measured — route B's engineering is settled, its decision is not

Two halves of route B were measured separately and never together: a CA
installed by a namespace-bound `policies.json` is trusted (above), and a
capsule's egress is a `CONNECT` tunnel through the daemon
(`docs/browser-capsule.md`). Whether Firefox accepts a certificate **minted for
the requested name and presented at the far end of a tunnel it opened itself**
was the one engineering unknown left, and a build that discovered the answer
halfway through would be the expensive place to find it.

It is measured, on Firefox 155.0, in the capsule's own sandbox shape — headless
`bwrap`, `--ro-bind / /`, `--proc /proc`, fresh profile, absolute
`--screenshot` path, the policy bound over
`/etc/firefox/policies/policies.json` inside the namespace only. A per-run
`openssl` CA signs a leaf for `intranet.example`; a python `CONNECT` proxy on
loopback answers `200`, terminates TLS with that leaf, adds
`Authorization: Bearer <marker>`, and re-originates to a plain-HTTP origin that
logs what it was sent. Firefox is pointed at the proxy by `network.proxy.type`
1 rather than by the environment, which is also why the capsule needs no
resolver: at type 1 it puts the NAME in the `CONNECT` and never looks it up.

| arm | leaf | CA installed | screenshot | reached the site | credential at the site | what the client said |
|---|---|---|---|---|---|---|
| authenticated | `intranet.example` | yes | 10402 B | yes | **yes** | handshake completed |
| control, no CA | `intranet.example` | no | 0 B | no | no | `TLSV1_ALERT_UNKNOWN_CA` |
| control, wrong name | `other.example` | yes | 0 B | no | no | `SSLV3_ALERT_BAD_CERTIFICATE` |

Both controls are needed and they fail differently, which is the point. Without
the second, a pass means only "Firefox rendered something": it is the
wrong-name arm that shows Firefox validating the **SAN** through the tunnel and
not merely accepting whatever the trusted CA signed. And the site's log is the
verdict alongside the screenshot — one request arrived, with the header, and it
came from a browser that never held the value.

So route B works, and what it costs in code is now the only engineering
question left. Two things the probe recorded that the design has to carry:

* **Firefox tells Mozilla.** With an enterprise root installed it reaches for
  `mitmdetection.services.mozilla.com` — the probe's proxy refused it, and a
  capsule under `--capability` refuses it too, because the pinned allowlist
  does not contain it. Worth knowing before somebody discovers it in a capsule
  that was allowed more.
* **Re-origination TLS was not the question and was not measured.** The origin
  here is plain HTTP on loopback. The daemon originating a validated TLS
  connection to the real site is ordinary client work it already does
  elsewhere, but it is not evidence from this probe.

Probe kept out of the tree deliberately, the way this page's cookie
measurement was: it needs a fixture CA, two servers and a browser for a
question that is asked once, and shipping it as a suite would put a MITM
harness in CI for a feature that does not exist.

### The thing it costs that is not code

**`/etc/firefox/policies/policies.json` is read inside every capsule.** That is
what the measurement above proves, and it is worth recording on its own: the
sandbox binds `/` read-only, so the machine's enterprise policy is part of
every capsule's trust surface. Today APEX's own file carries four preferences
at `Status: "default"` and nothing else, so nothing is overridden. A future
`Certificates.Install` there, or a `Proxy` at `Status: "locked"`, would change
what every capsule believes or where it connects, silently and without an
error anywhere.

`Containerfile.base` now asserts the shape, positively — `policies` carries
`Preferences` and nothing else, every preference at `Status: "default"` — so
all three of those additions fail the image build rather than shipping. It is a
build assertion and not a suite, because the file is dropped by the build and
not by anything `apex browser` owns; `tests/check-containerfile-assertions.sh`
runs it against this repository first, which before this round it could not do
for any `python3 -c` assertion at all.

### What it would take

* **A TLS server in `apex-agentd`.** `rustls` 0.23 is already a workspace
  dependency (`apex-remoted`, `apex-remote-core`) but only as a client.
  Certificate minting has no crate in the tree — `rcgen` is not a dependency —
  and `openssl` is in the image, which is how the browserlab already makes its
  own certificate.
* **Two protocol fields, and a `PROTOCOL_VERSION` bump of their own.** The
  session request would have to carry which destination is intercepted and
  which stored credential is presented. Both must be gated the way `--network
  offline` is gated, and `protocol.rs`'s own note says why: a daemon that
  predates a field ignores it. Here an old daemon would tunnel `CONNECT`
  untouched, present nothing, and hand back whatever the site says to an
  unauthenticated request — so the CLI must refuse to send the fields to a
  daemon below the revision rather than letting a capsule run and fail at the
  far end with a 401 nobody can explain.

  **Revision 11, and this page said 10 until gap 5 took it.** Protocol 9
  shipped for `RunRequest::allow` — the per-session allowlist narrowing that
  makes `--capability`'s destination pin a boundary rather than an
  announcement — and protocol **10** shipped for `RunRequest::trust_ca`, the
  per-run browser CA below. These fields need a number of their own for
  exactly the reason every other guard on that list has one: a daemon can
  understand a narrowed allowlist, and install a CA for a capsule's browser,
  and know nothing at all about terminating TLS for a destination. Reusing an
  earlier number would tell that daemon it understood a key it drops, which is
  the fail-open the version table exists for.
* **A sentence in `docs/browser-capsule.md` stops being true.** "A tunnel is
  opaque. This is a destination policy" becomes false for the pinned
  destination: the daemon reads the plaintext of a connection it is itself
  authenticating. That is not a new trust relationship — the daemon already
  holds the credential and already decides where the capsule may go, and it is
  more trusted than the capsule — but it is a documented property changing,
  and it has to change in the documentation at the same time.
* **Nothing new in the image.** `openssl` and `bwrap` are already there, and
  the probe above minted its whole chain with the `openssl` that ships.
* **Nothing unknown.** Before the probe, "Firefox accepts a minted leaf through
  a CONNECT tunnel" was an assumption this page was resting a route on. It is
  now a measurement with two controls. What remains is implementation and one
  product decision, in that order of difficulty — the decision is harder.

### What it does not solve

A site whose authentication is not a header. A login form, an OAuth redirect
chain, anything needing a browser to *do* something — Route B puts a credential
on a request and cannot fill in a form. For those, the answer is still a driver
in the capsule, which is Route C.

## Route C — a driver in the capsule

WebDriver, CDP or Marionette, handed a token for one request. It is the only
route that can drive a multi-step login, and it is blocked on a package: there
is no `geckodriver` and no WebDriver client in the image, and
`docs/browser-capsule.md` already records "no WebDriver, CDP or Marionette
surface" as not built for the same reason. It is not ranked against A and B
here because it is not a choice yet — it is a product decision about what the
image carries.

## Route D — a credential already in the URL

The `s3` provider mints SigV4 signatures today, and a presigned URL is a
short-lived credential a browser can use with no header and no driver at all.
For the class of sites that offer one, the work is a provider operation that
returns a URL and a capsule pointed at it — no protocol change, no TLS
termination, no CA. It is a paragraph rather than a route because it only ever
covers that class, and "capability auth" as a criterion is not about object
stores.

## The question, for Andre

Everything above is engineering that has been measured. This is the one thing
that is not, and route B cannot be built past it:

> **May `apex-agentd` read the plaintext of a capsule's connection to the one
> destination that capsule was pinned to, in order to add a credential the
> capsule is never given?**

What a *yes* buys: `apex browser --capability <name>` authenticates to any site
whose authentication is a header, with no per-site provider, and the six
negative observations on this page keep holding — the capsule still cannot be
made to hand a credential to its caller, because it never has one.

What a *yes* costs, itemised rather than waved at:

* A TLS **server** in `apex-agentd`, per-run certificate minting, and a CA that
  exists for the length of one capsule.
* `PROTOCOL_VERSION` **11** and two gated fields, with the CLI refusing to send
  them to an older daemon — an old daemon would tunnel `CONNECT` untouched and
  hand back a 401 nobody could explain. (This page costed it at 10 before gap 5
  was built; 10 is `RunRequest::trust_ca`.)
* `docs/browser-capsule.md` stops being able to say "a tunnel is opaque". For
  the pinned destination it is not.
* A guard in the daemon that the interception is only ever the pinned
  destination — an absence is not a guard. A proxy that can read one connection
  has the machinery to read any of them, and that is the honest argument
  against, not a hypothetical.

What a *no* costs: P2-012's "capability auth" stays unmet, permanently rather
than pending. Route A is already rejected on this page for handing the value to
the caller. Route C needs `geckodriver` in the image, which is a separate
product decision about what APEX carries. Route D covers presigned URLs and
nothing else. There is no fifth route that does not either give the capsule the
value or let the daemon see the request.

The decision is not urgent and it is not reversible cheaply, which is why it is
written here and not taken by an agent.

## What is not decided, and belongs to whoever picks this up

* ~~**Whether the per-run CA bind lands on its own.**~~ **Done, and it did.**
  `apex browser run --trust-ca FILE` and `apex agent run --trust-ca FILE`,
  `RunRequest::trust_ca` at protocol **10**, and
  `apex-agentd/src/browser_ca.rs`. It closed gap 5 — automating an intranet
  site behind a private CA — with none of route B, which is what made it
  dispatchable while the question below has no answer.

  The one decision inside it was taken the way this bullet asked: the flag
  **names the file** and has no default, because handing a capsule a CA it did
  not have widens what it will believe. Two things the build added that the
  bullet did not foresee. The machine's own
  `/etc/firefox/policies/policies.json` is **merged** rather than replaced — a
  capsule handed a document containing only a CA would have different browser
  defaults from every other browser on the machine, silently. And an absent
  policy file **refuses**: `--ro-bind-try` over a path that is not there is a
  no-op, so binding hopefully would produce a capsule that trusts nothing,
  renders nothing, and blames its own timeout.

  The probe above is the evidence that a real Firefox accepts such a root —
  its authenticated arm is exactly a capsule told to trust one CA. What the
  probe could not say is that the DAEMON builds the bind correctly, and
  `apexd/apex-agentd/tests/browser_ca_bind.rs` does: the session copies out
  what it sees at that path inside its own namespace, reads the path the
  document names, and copies that out too, with a control that sees the
  machine's own file byte for byte.
* ~~**A guard on the shipped `policies.json`.**~~ **Done.**
  `Containerfile.base` now asserts the file's shape and not only its validity,
  positively: `policies` carries `Preferences` and nothing else, and every
  preference is at `Status: "default"`. A denylist of `Certificates`, `Proxy`
  and `locked` was the wrong form — the file carries `//` comment keys, so a
  grep refusal would fire the day somebody explains in a comment why there is
  no `Certificates` block. Mutation-tested four ways, and
  `tests/check-containerfile-assertions.sh` runs it against this repository
  before an image build does, which it could not do for any `python3 -c`
  assertion until this round.

## What was measured for this page

| claim | how |
|---|---|
| a cookie written into a fresh profile is sent | four profiles on Firefox 155, including one that is the database alone; an untouched control |
| `expiry` is milliseconds, and a seconds value fails silently | the first probe, which drew the wrong conclusion from it |
| the engine cannot pass a value through the environment | the `authentication` flow's first control, which reached nothing |
| a value inside a capsule reaches the caller | the same flow's second control, through `--download` |
| a namespace-local `policies.json` installs a CA with no `certutil` | a self-signed loopback server, refused by the control profile and rendered by the policy one |
| `certutil` is absent | `nss-tools` is not installed |
| the host's enterprise policy is read inside a capsule | the same bind measurement, which is why it works |
| a minted leaf is accepted at the far end of a `CONNECT` tunnel, and the credential reaches the site | the three-arm probe above, on Firefox 155.0 |
| Firefox validates the chain through the tunnel | the no-CA control: `TLSV1_ALERT_UNKNOWN_CA`, no screenshot, no request at the origin |
| and validates the name, not merely the signature | the wrong-name control: `SSLV3_ALERT_BAD_CERTIFICATE`, same |
| the daemon's own bind puts the merged policy at that path inside a capsule, and the file it names is openable from in there | `apex-agentd/tests/browser_ca_bind.rs`, where the SESSION copies out what it sees; the control with no `--trust-ca` sees the machine's file byte for byte |
