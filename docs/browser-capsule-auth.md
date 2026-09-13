# Can a browser capsule log in? — the decision, and what it costs

P2-012's acceptance line is "isolated browser profile/cookies/downloads and
capability auth". Three of those four are built and measured
(`docs/browser-capsule.md`). The fourth is not: **a capsule cannot present a
credential to a site**, and that is why the item is recorded `partial`.

This page is the written decision the gap needs before anybody writes code for
it. It says what the framework guarantees today, what each route would
change, which measurement kills or permits each one, and what the chosen route
would cost. Nothing here is built.

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
allowlist — but they bound it *after* accepting that the caller ends up
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

That makes gap 5 on this unit's card — "a CA a capsule could be told to trust" —
a per-session `--ro-bind` in `apex_agent_core::sandbox`, which is one field on
the session request rather than a package that is not in the image.

### The thing it costs that is not code

**`/etc/firefox/policies/policies.json` is read inside every capsule.** That is
what the measurement above proves, and it is worth recording on its own: the
sandbox binds `/` read-only, so the machine's enterprise policy is part of
every capsule's trust surface. Today APEX's own file carries four preferences
at `Status: "default"` and nothing else, so nothing is overridden. A future
`Certificates.Install` there, or a `Proxy` at `Status: "locked"`, would change
what every capsule believes or where it connects, silently, and no test asserts
the file's shape. See "what is not decided" below.

### What it would take

* **A TLS server in `apex-agentd`.** `rustls` 0.23 is already a workspace
  dependency (`apex-remoted`, `apex-remote-core`) but only as a client.
  Certificate minting has no crate in the tree — `rcgen` is not a dependency —
  and `openssl` is in the image, which is how the browserlab already makes its
  own certificate.
* **Two protocol fields, and a `PROTOCOL_VERSION` bump.** The session request
  would have to carry which destination is intercepted and which stored
  credential is presented. Both must be gated the way `--network offline` is
  gated, and `protocol.rs`'s own note says why: a daemon that predates a field
  ignores it. Here an old daemon would tunnel `CONNECT` untouched, present
  nothing, and hand back whatever the site says to an unauthenticated request —
  so the CLI must refuse to send the fields to a daemon below the revision
  rather than letting a capsule run and fail at the far end with a 401 nobody
  can explain.
* **A sentence in `docs/browser-capsule.md` stops being true.** "A tunnel is
  opaque. This is a destination policy" becomes false for the pinned
  destination: the daemon reads the plaintext of a connection it is itself
  authenticating. That is not a new trust relationship — the daemon already
  holds the credential and already decides where the capsule may go, and it is
  more trusted than the capsule — but it is a documented property changing,
  and it has to change in the documentation at the same time.
* **Nothing new in the image.** `openssl` and `bwrap` are already there.

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

## What is not decided, and belongs to whoever picks this up

* **Whether the MITM is acceptable at all.** It is the load-bearing choice and
  it is a product decision, not an engineering one. The argument for it is
  above; the argument against it is that a proxy that can read one destination
  is a proxy that has the machinery to read any of them, and the guard against
  that is a check in the daemon rather than an absence.
* **A guard on the shipped `policies.json`.** Nothing asserts that the file has
  no `Certificates`, no `Proxy` and no `Status: "locked"` preference. It should,
  and the assertion belongs beside the other Containerfile assertions rather
  than in this unit's suite, because the file is dropped by the image build and
  not by anything `apex browser` owns.
* **Whether the per-run CA bind lands on its own.** It closes gap 5 —
  automating an intranet site behind a private CA — without any of Route B. It
  is smaller than Route B and useful without it, and it carries its own
  argument: handing a capsule a CA it did not have is widening what it will
  believe, and the flag has to name the file rather than defaulting to
  anything.

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
