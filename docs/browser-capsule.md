# The browser automation capsule

`apex browser` runs a browser that automates a site without letting it near the
browser you use yourself.

P2-012's acceptance line is "isolated browser profile/cookies/downloads and
capability auth". This page records what each of those four words rests on,
what was measured rather than assumed, and — in the last section — what is not
built.

## It is a composition, not a new boundary

Nothing here invents confinement. Every property below is an existing APEX
guarantee pointed at a browser:

| the property | what it rests on |
|---|---|
| the profile you use is unreachable | the agent sandbox's `--tmpfs $HOME` (`apex-agent-core/src/sandbox.rs`) |
| headless is structural, not a flag | the same sandbox's `--tmpfs /run` and masked `$XDG_RUNTIME_DIR`: there is no compositor socket to open a window on |
| the browser reaches only named hosts | `NetworkPolicy::Allowlist` — `--unshare-net` plus agentd's CONNECT proxy, which asks `destination::Allowlist` twice (`apex-agentd/src/egress.rs`) |
| ignoring the proxy opens nothing | the same `--unshare-net`: the namespace has only `lo`, so there is no route to try and no resolver to ask |
| downloads leave only when nominated | `apex vm run`'s egress rule — the loop runs over the **nominations**, never over the directory's contents |
| nothing survives the run | `apex vm run`'s four-way fence on a recursive removal: narrow name pattern, final component not a symlink, `realpath`, and the resolved path equal to exactly `<root>/<name>` |

`apex browser` is a client of `apex agent` rather than a second sandbox. It builds
a capsule directory and a profile, asks the runtime for a confined allowlisted
session whose working directory is that capsule, waits for it, copies out the
files that were nominated, and deletes the capsule.

## Why not a VM

`apex vm run` is the stronger boundary and it is the wrong one here, for three
reasons that are recorded rather than argued:

* **No guest image carries a browser.** P2-009 landed `partial` for the same
  reason one step over: no guest image carries an agent CLI either, and
  `docs/virtualization.md` says building one is separate work.
* **`apex vm run` refuses `--network` on purpose**, because an outbound
  interface would make its file-egress boundary decorative. A browser with no
  network is not a browser, so a VM-tier browser capsule is not that verb with
  a flag added — it is a different design, with a different argument about what
  the network is allowed to be.
* **The virt stack is deliberately not in the image** and
  `Containerfile.base` asserts its absence. `bwrap`, `apex-agentd` and Firefox
  are all in the image, so the capsule described here works on a stock machine
  with nothing installed.

A VM-tier browser capsule is listed under "what is not built".

## Checking the machine can run one

```
apex browser doctor
```

Reports each part a capsule needs and names the one that is missing rather than
failing halfway through a run: the browser binary (present, present-but-not-
executable, or missing), `apex` itself, and whether the agent runtime answers —
which is the one a fresh machine is most likely to be missing, so it prints the
`systemctl --user enable --now apex-agentd` that fixes it. Exits non-zero when
anything is absent, so it is usable as a precondition in a script.

## The capsule

```
apex browser run --allow example.com:443 \
    --download page.png --download-to /home/u/results \
    -- --screenshot {capsule}/page.png https://example.com/
```

`{capsule}` in any browser argument becomes the capsule's own directory. It is
not a convenience: Firefox's `--screenshot` writes **nothing at all** when it
is given a relative filename — measured, and the run still exits 0 — and a
caller cannot type the path of a capsule this command names for them. A token
rather than a rewrite of `--screenshot`, because the engine does
not know which of a browser's flags take paths and guessing would be a list to
keep correct for every browser and every version.

A run makes one directory, `$XDG_STATE_HOME/apex/browser/<name>`, mode `0700`,
and everything the browser is allowed to keep lives in it:

```
<capsule>/.profile/    the browser profile — fresh, and only ever this one
<capsule>/             where the browser puts everything else it writes
```

That directory is the session's working directory, which is the one host path
a confined session gets bound writable. It is **not** the home: the home inside
the capsule is a tmpfs, so `~/.mozilla` resolves to an empty directory that the
browser creates for itself and that dies with the process.

**The capsule directory is also the download directory**, and that is a
correction rather than a design. The first version put downloads in a
`downloads/` subdirectory, and the live lab caught what that costs:
`--screenshot shot.png` writes `shot.png` to the browser's working directory,
which is the capsule root, so the nomination loop looked one level down and
reported that the capsule "did not produce" a file it was standing next to. A
caller would have had to know the engine's internal layout to nominate anything
the browser wrote for itself.

The profile is `.profile` rather than `profile` for a reason that is not
cosmetic: a nomination may not start with a dot, so the profile cannot be
nominated at all — by construction, rather than by a check somebody has to
remember to keep.

### The profile is fresh, and there is no flag that says otherwise

`apex browser` writes a new profile for every run. There is no `--profile`
naming one you already have, and no way to point it at `~/.mozilla`: the path
is derived from the capsule name, and the real profile is not in the mount
namespace to be pointed at.

That is a scope decision as much as a security one. A profile that persisted
between runs would be long-lived credential storage — a cookie jar that an
automation accumulates and that nothing expires — which is the thing capability
auth exists to replace. Persistence is listed under "what is not built" so that
adding it later has to argue for itself.

### Cookies

Cookies are a file in the profile, so they inherit the profile's lifetime: they
are created inside the capsule and deleted with it. The interesting claim is
the negative one — that the capsule cannot read the cookies of the browser you
use — and it holds because of the masked home rather than because the capsule
is careful. There is no code path that could copy them in.

**A cookie jar cannot be carried out either**, and that follows from the
layout rather than from a rule somebody wrote. Cookies live in the profile,
the profile is `.profile`, and a nomination may not start with a dot or carry
a directory component — so there is no filename that names the cookie jar. A
capsule's session ends with the capsule.

That is the stronger position and it is the one this verb takes. Handing a
session back to the host is the thing a persistent profile would be for, and
persistence is listed under "what is not built" for the same reason: a cookie
jar an automation accumulates and nothing expires is long-lived credential
storage, which is what capability auth exists to replace.

### Downloads

The profile pins the download directory to the capsule and turns off the "ask
me where" dialogue, because a headless browser has nobody to ask.

Downloads reach the host exactly the way a disposable VM's files do:

* `--download NAME` nominates a plain filename. No directory component, no
  `..`, no wildcard — a wildcard would let the browser choose what leaves by
  choosing a filename.
* `--download-to DIR` is the only thing that makes anything leave at all.
  Without it nothing does, however much is nominated.
* The copy loop runs over the nominations. It never lists the capsule and
  copies what it finds, which is the same direction `apex vm run` documents as
  its security property.
* A nominated file that would land on top of an existing one is not copied
  unless `--force` says so. The file was produced by a page the caller did not
  trust enough to open in their own browser; replacing something of theirs with
  it is a decision.
* `--download-to` is refused if it resolves inside the capsule root, because
  teardown deletes that tree and every file would be reported copied and then
  removed.

## The network

A capsule with no `--allow` is refused. The reason is the one
`AgentPolicy::validate_for` already gives for an allowlisted session with an
empty allowlist: a browser that can reach nothing while reporting a destination
policy is a mode that lies.

Each `--allow` destination must already be on the runtime's allowlist
(`apex agent allow <host>`), and the refusal names the exact line to add. This
is a declaration checked against the set the daemon enforces, not a second
grant path: the allowlist agentd snapshots when the session starts is the
runtime's, and `apex browser` cannot widen it.

The browser is pointed at the in-namespace bridge with profile preferences
rather than `HTTP_PROXY`, because Firefox does not read the environment
variables. `network.proxy.type=1` and the http/ssl proxy host and port are
written into the profile by the engine, which makes those preferences part of
the security surface and not cosmetic — the suite asserts them.

Ignoring them buys nothing, and the lab measures it rather than quoting it. A
capsule's interface list is `lo` and nothing else, so a connection aimed past
loopback has no route to try: `curl` to an address in TEST-NET-2 fails to
connect in 0 ms, and a connection by *name* does not get that far, because
there is no resolver in the namespace at all — the daemon is what resolves,
which is also why the allowlist checks a name rather than an address.

That the **browser** uses the bridge, and not only that `curl` can, has its own
evidence and it comes from the enforcer: a capsule pointed at a destination
nobody allowed produces a denial in agentd's own log naming that host, and
nothing but a proxied request could have produced it.

Two consequences, stated because they are limits rather than holes:

* **The proxy tunnels `CONNECT` only.** A plain `http://` URL is refused by the
  daemon with a 405, so a capsule talks to https sites.
* **A tunnel is opaque.** This is a destination policy: a capsule allowed to
  reach a host can send that host anything.

## The browser talks to its vendor, and the allowlist is what stops it

A capsule's profile turns off telemetry, the updater, remote settings, the
region and geolocation services, the captive-portal and connectivity probes,
safebrowsing and prefetch. That is hygiene, and it is not sufficient: with all
of it applied, the lab's daemon still logs a capsule being denied
`firefox.settings.services.mozilla.com` and `aus5.mozilla.org`.

**The allowlist is the boundary and the preferences are not.** A capsule reaches exactly the destinations it was
allowed, whatever the browser decides it would like to contact, and the daemon
writes down every refusal. The preferences exist so that a run's allowlist
describes what the capsule talks to, near enough. Nothing depends on them.

## Firefox keeps its own sandbox, and that needed a measurement

Firefox confines its own content processes with a user namespace, which means
writing `/proc/self/uid_map` from inside APEX's sandbox. Measured rather than
assumed: with `/proc` inherited read-only from the host, that write fails
`EROFS`, every content process dies on `SIGSEGV`, and the run produces no
output at all while the parent exits 0 — a silent nothing.

The agent sandbox already mounts a fresh procfs (`--proc /proc`, beside
`--unshare-pid`), so the nesting works and the browser's own sandbox survives
inside APEX's. The capsule therefore has two boundaries, not one, and the
suite asserts the fresh `/proc` is there, so removing it fails a test instead
of removing a layer with nobody noticing.

## Headless, and there is no flag to make it otherwise

`apex browser` has no `--headed`, no `--display` and no viewer verb, the same
way `apex vm` has no `--graphics`. The absence is asserted rather than
reviewed.

The guarantee is structural: the capsule's `/run` and
`$XDG_RUNTIME_DIR` are tmpfs, so `$WAYLAND_DISPLAY` names a socket that does
not exist. A browser told to open a window in there cannot find a compositor.
`MOZ_HEADLESS` is set as well, but it is the belt and the missing socket is the
braces — a capsule whose `MOZ_HEADLESS` was unset still cannot put a window on
anybody's screen.

## Capability auth: what it decides, and what it does not

`--capability NAME` names a credential already in `apex-secretd`'s root-owned
store. It binds the capsule's destination to that credential's **pin**:

* the run is refused if no credential of that name is stored;
* the capsule's allowed destination is the host **and the port** the credential
  was pinned to when it was stored, not a destination typed at the call site;
* `--allow` naming any other host is refused, by name, rather than added, and so
  is the same host on a different port — a different port is a different
  endpoint.

The pin is exercised against a real `apex-secretd` in
`tests/browserlab/run-browserlab`'s `capability` flow, with the falsifying
control the claim needs: an engine that looks the capability up and then does
not apply it sends the capsule wherever the call site asked.

That flow exists because the first version of this could not work at all. It
read the pin out of the second column of `apex secret list`, which is
`scheme://host` and has no port in it, so against a real secret service the
destination came out as `https://intranet.example` and every capsule naming a
capability was refused. The suite did not catch it because its stub printed a
bare host there — a stub that agreed with the engine rather than with the CLI.
The pin is read from `apex secret list --json` now, which is the spelling with
the fields in it.

The capability decides where the browser may go. That is the half of
§13's model a browser can use without overstating it: the framework's rule is
that a caller does not get to choose where a credential's authority is spent,
and here the caller does not get to choose where the capsule can reach.

**The capsule is never given the credential's value.** It is not written into
the profile, not put in the environment, and not passed on the command line.
`apex secret list` never prints a value and this engine never asks for one.

What follows from that is the honest limit: **a capsule cannot log in to a
site.** The framework exists so that an agent uses a credential without
holding it, and it does that by performing the operation itself — which works
for a git push and an MCP request and does not work for a browser, because
agentd's proxy tunnels `CONNECT` and a TLS tunnel is opaque to the thing
carrying it. There is nowhere for the daemon to insert a header.

Presenting a minted, short-lived credential to a site from inside a capsule is
therefore not built. **`docs/browser-capsule-auth.md` is the written decision**
about how it would be: the two routes, what each one costs, the measurements
that permit or reject each, and the protocol change the chosen one needs.

The short version, because it decides what this page promises. A capsule's
**profile is the only thing an engine can put inside it** — a session's
environment is built by the daemon and does not inherit the caller's, measured
by a control that tried and reached nothing. And **anything a capsule can see,
the caller can take home**, because `--download` is a path out of the capsule by
design. So handing the capsule a session would hand the caller the credential,
which is the thing the framework exists to prevent; the route that keeps it out
of the capsule is for the daemon to present the header itself, and that is a
change to the proxy rather than a flag on this command.

## What is not built

* **A capsule cannot authenticate to a site.** The paragraph above says why,
  and it is the reason P2-012 is recorded `partial` rather than `done`.
* **No VM-tier capsule.** The three reasons are at the top of this page.
* **No persistent profile.** Deliberate; see above.
* **No browser other than Firefox.** The profile format, the proxy
  preferences and the download preferences are Firefox's. A Chromium capsule
  would share the sandbox, the allowlist and the nomination loop and would need
  its own profile writer.
* **No WebDriver, CDP or Marionette surface.** A run hands its arguments to the
  browser, so `--screenshot` works and a scripted multi-step interaction does
  not. Driving a page needs a driver in the capsule, which needs a package that
  is not in the image.
* **The proxy is `CONNECT`-only**, so a plain-http site cannot be automated.
* **A capsule trusts the system CA store and nothing else, and there is no way
  to add to it.** A fresh profile has a fresh `cert9.db`, so a site behind a
  private or self-signed certificate is refused by the browser before any of
  this page's boundaries come into it. Measured rather than reasoned: the live
  lab's own HTTPS server is self-signed, and pointing a capsule's browser at it
  produced no page and no screenshot — the browser sat on the refusal until the
  capsule timed out. The lab now renders from a `data:` URL for anything that
  needs a page, and reaches its server with `curl -k`, so that what it measures
  is the capsule rather than a certificate.

  **The mechanism is known and it is not `certutil`.** `nss-tools` is not in
  the image, which was recorded as closing the question, and it does not: a
  `policies.json` bound over `/etc/firefox/policies/policies.json` inside the
  capsule's namespace installs a CA into a fresh profile with no `certutil` at
  all. Measured with a control — the same server, the same profile, refused
  without the policy and rendered with it, and the machine's own file untouched.
  So this is a per-session `--ro-bind` on the sandbox rather than a package
  that does not exist here. See `docs/browser-capsule-auth.md`.

  It is still not built, and adding it is a real flag with a real argument
  behind it — a CA is trust, and handing a capsule a CA it did not have is
  widening what it will believe. A capsule silently trusting more than the
  system does would be worse than the gap.

* **The machine's own Firefox enterprise policy is read inside every capsule**,
  because the sandbox binds `/` read-only — which is exactly why the bind above
  works. Today `/etc/firefox/policies/policies.json` carries four preferences at
  `Status: "default"` and nothing else, so nothing a capsule does is overridden.
  A `Certificates.Install` there, or a `Proxy` at `Status: "locked"`, would
  change what every capsule believes or where it connects, and nothing asserts
  the file's shape. Recorded here rather than guarded, because the file belongs
  to the image build and not to this command.
