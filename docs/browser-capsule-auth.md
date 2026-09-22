# Can a browser capsule log in? — the decision, and what it costs

P2-012's acceptance line is "isolated browser profile/cookies/downloads and
capability auth". All four are now built and measured: the first three in
`docs/browser-capsule.md`, and the fourth — **a capsule presenting a credential
to a site** — here.

This page started as the written decision the gap needed before anybody wrote
code for it. It stays because the argument is the record: what the framework
guarantees, what each route would change, which measurement kills or permits
each one, and what the chosen route cost. Everything below is kept in the order
it was decided in, with what was built marked where it was decided.

**Route B is built** (`--present`, protocol **11**), and the question at the
bottom of this page — the one it was blocked on — has been answered. Route A is
rejected on this page for a measured reason. Routes C and D are unbuilt and
need a package and a provider operation respectively. One other thing here is
built and is not a route: the **per-run CA bind** (`--trust-ca`, protocol 10),
which lets a capsule automate an intranet site behind a private root.

## What is decided

**The route is B — the daemon presents the credential and the capsule never
holds it.** Route A, handing the capsule a session, is rejected, and the reason
is a measurement rather than a preference: a value inside a capsule reaches the
caller through the nomination boundary, which is a path the capsule's own
design puts there on purpose.

**Route B is built.** `apex browser run --capability NAME --present`,
`apex agent run --present NAME`, `RunRequest::present` at protocol 11,
`apex-agentd/src/intercept.rs` and `apex-secretd/src/present.rs`. What it took
is at "What it would take" below, rewritten as what it took; the decision that
unblocked it is at "The question, for Andre", which is now answered.

It did change a sentence `docs/browser-capsule.md` states as a property — "a
tunnel is opaque" — and that page now states the bounded version: opaque for
every destination except the one the session named a credential for.

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
terminates TLS with a certificate it mints for the run and hands the plaintext
to `apex-secretd`, which adds the credential's header and originates its own
TLS connection to the site. The capsule trusts a CA that exists for the length
of one capsule and is trusted by nothing else on the machine.

**Which daemon adds the header is not a detail, and this page had it wrong.**
It said, under "What it would take", that the change was "not a new trust
relationship — the daemon already holds the credential". `apex-agentd` does
not and must not: it runs as the user, so a value it held an unconfined session
of that user could read, and `apex-agentd/src/broker.rs` states the invariant
this rests on — no verb in `apex_secret_core::protocol` returns a credential,
and nothing in that daemon holds a `SecretValue`. The build kept that sentence
true by splitting the work: the runtime terminates TLS and pumps plaintext, and
the one process that ever sees the value is the one that already held it,
already decided where it could be spent, and already sees the plaintext of
every `apex secret use`.

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
what the build found that this paragraph did not foresee. Route B installs its
own minted authority through the same mechanism, which is why `--trust-ca` and
`--present` are refused together: a capsule with `--present` has one
destination and the runtime terminates it, so a caller-supplied root would be
for a connection that no longer exists.

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
* ~~**Re-origination TLS was not the question and was not measured.**~~
  **Measured, in the build.** The origin in this probe is plain HTTP on
  loopback, so the probe said nothing about it. The shipped path does:
  `apex-secretd` opens a validated `rustls` connection to the site against the
  machine's own trust store, and `egress.rs`'
  `a_capsules_request_reaches_the_site_with_a_credential_it_is_never_given`
  exercises it against a loopback TLS origin whose authority the test minted,
  named to the daemon through `SSL_CERT_FILE` — the variable
  `rustls-native-certs`, `curl` and `git` all read, rather than a hook asked
  for by the test.

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

### What it took

Written as an estimate before the build and rewritten after it, with the two
places the estimate was wrong marked rather than quietly corrected.

* **A TLS server in `apex-agentd`** — `apex-agentd/src/intercept.rs`. `rustls`
  0.23 was already a workspace dependency but only as a client; it is a
  dependency of this crate now, pinned to the same version `apex-remoted` and
  `apex-secretd` use so one image cannot hold two rustls builds. Certificate
  minting still has no crate in the tree and still does not need one: a CA and
  one leaf per session, from the `openssl` the image ships, elliptic-curve
  rather than RSA because a 2048-bit keygen is a visible pause on every capsule.
  The CA's private key is deleted the moment the leaf is signed.

  **ALPN is `http/1.1` and nothing else**, which the estimate did not foresee.
  Firefox offers h2 through a `CONNECT` tunnel, and an h2 stream would be
  carried as frames nothing adds a header to — a silent 401 at the far end.

* **ONE protocol field, not two, and a `PROTOCOL_VERSION` bump of its own.**
  The estimate said the request "would have to carry which destination is
  intercepted and which stored credential is presented". It carries only the
  credential. Where it is spent is that credential's own pin, read out of the
  secret service; a second field naming the destination would be a second thing
  that can disagree with the first, and the request would settle the
  disagreement in favour of whichever one the caller wrote. What the daemon
  requires instead is that the session's narrowed allowlist BE that
  destination — exactly one rule, that host, that port.

  **Revision 11, and this page said 10 until gap 5 took it.** Protocol 9
  shipped for `RunRequest::allow` — the per-session allowlist narrowing that
  makes `--capability`'s destination pin a boundary rather than an
  announcement — and protocol **10** shipped for `RunRequest::trust_ca`, the
  per-run browser CA below. This field needs a number of its own for exactly
  the reason every other guard on that list has one: a daemon can understand a
  narrowed allowlist, and install a CA for a capsule's browser, and know
  nothing at all about terminating TLS for a destination. Reusing an earlier
  number would tell that daemon it understood a key it drops, which is the
  fail-open the version table exists for. `BROWSER_CA_VERSION <
  BROWSER_PRESENT_VERSION` is asserted at compile time, so a build that
  renumbered them does not link.

* **A sentence in `docs/browser-capsule.md` stopped being true, and the
  replacement is narrower than the estimate expected.** "A tunnel is opaque.
  This is a destination policy" is still true of every destination but one. For
  the pinned one the runtime reads the request and `apex-secretd` reads the
  request and the answer.

  The estimate justified that with "the daemon already holds the credential",
  **and that was wrong** — see the correction at the top of this route.
  `apex-agentd` holds no credential before this change and holds none after it.
  The property `apex-secretd`'s protocol note states — no verb returns a
  credential — is still true, and the split between the two daemons is what
  kept it true rather than a comment claiming it.

* **A guard, which is most of the work.** Refused at session start: an
  unconfined sandbox (no namespace, so the minted authority could not be
  installed), a network policy that is not `allowlist` (no egress proxy, so
  nothing would intercept), `--trust-ca` alongside, and an allowlist that is
  not exactly the pin. In the proxy: exact equality on host and port, not a
  question for the allowlist, whose rules can be wildcards. In
  `apex-secretd`: the grant, the operation (`browser.present` and nothing
  else), and the pin again — because the runtime runs as the user and is not
  the boundary.

* **A grant vocabulary entry**, which the estimate did not foresee at all.
  `apex-secretd` refuses to write a grant for an operation no provider offers,
  so there is a `browser` provider whose one operation, `browser.present`, both
  trait methods refuse: it exists so that `apex secret grant NAME
  browser.present --everywhere` can be typed, and an interception with no grant
  behind it would be a way for anything running as the user to spend a
  credential on arbitrary requests to the pinned host. `--everywhere`, because
  a capsule's working directory is a throwaway tree and a project-keyed grant
  would match one capsule and then name a path that is gone.

* **One request per connection.** `Connection: close` is forced onto the
  rewritten head so the far side cannot keep the connection alive and send a
  second request the relay would have to parse; chunked request framing is
  refused rather than parsed, and a body is carried by its declared length.
  A browser meets a closed connection by opening another, which arrives as
  another `Present` with the grant checked again. The parsing surface is one
  head, in a root process, reading bytes a capsule's browser chose.

* **The response direction is scrubbed.** Not foreseen either, and it is what
  makes "the capsule never holds the credential" survive a site that echoes the
  header back: the stream is scanned for the value and any occurrence replaced
  with `x` of the same length, because the site declared a `Content-Length` the
  browser will read to the end of. It cannot catch a site that transforms the
  value first — a credential spent at a site is a credential that site has.

* **Nothing new in the image.** `openssl` and `bwrap` were already there, and
  the build minted its whole chain with the `openssl` that ships.

### What it does not solve

A site whose authentication is not a header. A login form, an OAuth redirect
chain, anything needing a browser to *do* something — Route B puts a credential
on a request and cannot fill in a form. For those, the answer is still a driver
in the capsule, which is Route C.

Four more limits, stated because they are limits and not holes:

* **A site that transforms the credential before echoing it** gets past the
  response scrub. The scrub catches the ordinary case — a debug endpoint, an
  error page quoting the request — and cannot catch base64 or a hash. A
  credential spent at a site is a credential that site has.
* **`SessionInfo` does not carry `present`**, exactly as it does not carry
  `trust_ca`. `apex agent status` cannot show that a session's one destination
  is being authenticated. For a field that decides whether a capsule is logged
  in, a record that is silent about it is a gap; an additive optional field on
  a stability surface is a follow-up, not part of this.
* **The pinned site sees a TCP connection for a request that is refused.**
  `apex-secretd` opens its connection to the site before it has read the
  capsule's head, which is `CONNECT` semantics — the tunnel is established and
  then spoken on. A request refused by the head rewrite leaves a socket that
  was accepted and never used. An empty entry in a site's log is not a leak.
* **No Firefox has run through the shipped proxy.** Round 29's probe measured a
  real Firefox accepting a minted leaf at the far end of a `CONNECT` tunnel and
  validating the SAN through it; this round measured what the DAEMON does, with
  a `rustls` client in its place. The join is by construction — the same policy
  bind, the same kind of leaf, the same tunnel — and not by a browser run.

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

## The question, for Andre — answered, yes

Everything above was engineering that had been measured. This was the one thing
that was not, and route B could not be built past it:

> **May `apex-agentd` read the plaintext of a capsule's connection to the one
> destination that capsule was pinned to, in order to add a credential the
> capsule is never given?**

**Andre said yes.** Relayed to this unit in its round-31 dispatch on
**2026-09-19**; the date he said it is not recorded here, because this page
does not know it and inventing one would be worse than the gap.

What a *yes* bought, as promised: `apex browser run --capability NAME
--present` authenticates to any site whose authentication is a header, with no
per-site provider, and the six negative observations on this page keep holding
— the capsule still cannot be made to hand a credential to its caller, because
it never has one.

What it cost, against the estimate that was put to him:

* A TLS **server** in `apex-agentd`, per-run certificate minting, and a CA that
  exists for the length of one capsule. **As estimated.**
* `PROTOCOL_VERSION` **11** and gated fields with the CLI refusing to send them
  to an older daemon. **One field, not two** — see "What it took".
* `docs/browser-capsule.md` stops being able to say "a tunnel is opaque". **For
  the pinned destination only**, which is narrower than the estimate and is how
  that page now states it.
* A guard in the daemon that the interception is only ever the pinned
  destination. **Built, and it is most of the work** — the estimate called it
  "a guard", and it is four refusals at session start, an exact-equality test in
  the proxy, and three more checks in `apex-secretd` because the runtime runs
  as the user and is not the boundary.

### What the answer did NOT cover, and what was done about it

The question was about the runtime reading **plaintext**. It was not a decision
to give the runtime a **credential**, and those are different questions:
`apex-agentd` runs as the user, so a value it held an unconfined session of
that user could read — which is the whole reason the store moved to
`apex-secretd` under P0-002.

So the build was arranged so that the second question never had to be asked.
The runtime terminates TLS toward the capsule and pumps plaintext; the
credential is added by `apex-secretd`, which already held it. **No verb in
`apex_secret_core::protocol` returns a credential** — the sentence
`apex-agentd/src/broker.rs` states as a property — is as true after this round
as before it.

That is recorded here rather than buried in a commit because it is the place
Andre would look to object. If what he meant was "and the runtime may hold the
value too", nothing was lost by not doing it; if he meant what this build did,
the record says so.

## What is not decided, and belongs to whoever picks this up

* ~~**Whether the per-run CA bind lands on its own.**~~ **Done, and it did.**
  `apex browser run --trust-ca FILE` and `apex agent run --trust-ca FILE`,
  `RunRequest::trust_ca` at protocol **10**, and
  `apex-agentd/src/browser_ca.rs`. It closed gap 5 — automating an intranet
  site behind a private CA — with none of route B, which is what made it
  dispatchable while the question above had no answer.

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
* **What is left, after route B.** Three things, none of them blocking:
  `SessionInfo` carries neither `trust_ca` nor `present`, so `apex agent
  status` cannot show that a capsule trusts an extra root or that one of its
  destinations is authenticated — an additive optional field on a stability
  surface, and a follow-up. Route C still needs `geckodriver` in the image,
  which is a product decision about what APEX carries and is what a form login
  waits on. And round 4's recorded inconsistency is still there: the engine's
  pre-check falls back to the bare host, so `--allow e.example:8443` passes it
  when only `e.example` is allowed and the DAEMON refuses it. Fail-closed; the
  two just do not agree.
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
| the runtime terminates the PINNED destination and tunnels every other one | `egress.rs`'s `the_pinned_destination_is_terminated_and_every_other_stays_an_opaque_tunnel`: two loopback TLS origins with an authority each, and a client trusting one root set at a time. The pin verifies against the per-run CA and not against the origin's own; another allowed destination verifies against the origin's own and not against the per-run CA. The pairs are what give either half meaning |
| a session that named no credential has nothing terminated | the same test's control arm: the pin reaches its own certificate |
| the credential reaches the site and not the capsule, in one run | `egress.rs`'s `a_capsules_request_reaches_the_site_with_a_credential_it_is_never_given`, against a private `apex-secretd` and a site that echoes the header back on purpose — the echo comes back redacted in place, same length |
| the runtime originates a validated TLS connection to the site | the same test: the site's certificate is signed by an authority the test minted and named through `SSL_CERT_FILE` |
| a capsule's own `Authorization` header never reaches the site | `apex-secretd/tests/present.rs`, through the real relay |
| no grant means no connection | the same file: the refusal AND an empty log at the site, because "refused" and "reached the site and the site said no" are otherwise the same observation |
| the capsule's operation is the only one this path can spend | the same file: a record naming `git.push`, granted, refused |
