# The browser automation capsule

`apex browser` runs a browser that automates a site without letting it near the
browser you use yourself.

P2-012's acceptance line is "isolated browser profile/cookies/downloads and
capability auth". This page records what each of those four words rests on,
what was measured instead of assumed, and, in the last section, what is not
built.

## A composition of existing boundaries

Nothing here invents confinement. Every property below is an existing APEX
guarantee pointed at a browser:

| the property | what it rests on |
|---|---|
| the profile you use is unreachable | the agent sandbox's `--tmpfs $HOME` (`apex-agent-core/src/sandbox.rs`) |
| headless is structural, not a flag | the same sandbox's `--tmpfs /run` and masked `$XDG_RUNTIME_DIR`: there is no compositor socket to open a window on |
| the browser reaches only named hosts | `NetworkPolicy::Allowlist`: `--unshare-net` plus agentd's CONNECT proxy, which asks `destination::Allowlist` twice (`apex-agentd/src/egress.rs`) |
| ignoring the proxy opens nothing | the same `--unshare-net`: the namespace has only `lo`, so there is no route to try and no resolver to ask |
| downloads leave only when nominated | `apex vm run`'s egress rule: the loop runs over the **nominations**, never over the directory's contents |
| nothing survives the run | `apex vm run`'s four-way fence on a recursive removal: narrow name pattern, final component not a symlink, `realpath`, and the resolved path equal to exactly `<root>/<name>` |

`apex browser` is a client of `apex agent`, not a second sandbox. It builds a
capsule directory and a profile, asks the runtime for a confined allowlisted
session whose working directory is that capsule, waits for it, copies out the
files that were nominated, and deletes the capsule.

## Why not a VM

`apex vm run` is the stronger boundary and the wrong one here, for three
recorded reasons:

* **No guest image carries a browser.** P2-009 landed `partial` for the same
  reason one step over: no guest image carries an agent CLI either, and
  `docs/virtualization.md` says building one is separate work.
* **`apex vm run` refuses `--network` on purpose**, because an outbound
  interface would make its file-egress boundary decorative. A browser with no
  network is not a browser, so a VM-tier browser capsule is not that verb with
  a flag added. It is a different design, with a different argument about what
  the network is allowed to be.
* **The virt stack is not in the image**, and `Containerfile.base` asserts its
  absence. `bwrap`, `apex-agentd` and Firefox are all in the image, so the
  capsule described here works on a stock machine with nothing installed.

A VM-tier browser capsule is listed under "what is not built".

## Checking the machine can run one

```
apex browser doctor
```

Reports each part a capsule needs and names the one that is missing, so a run
does not fail halfway through: the browser binary (present, present but not
executable, or missing), `apex` itself, and whether the agent runtime answers.
A fresh machine is most likely to be missing the runtime, so for that one it
prints the `systemctl --user enable --now apex-agentd` that fixes it. It exits
non-zero when anything is absent, so you can use it as a precondition in a
script.

## The capsule

```
apex browser run --allow example.com:443 \
    --download page.png --download-to /home/u/results \
    -- --screenshot {capsule}/page.png https://example.com/
```

`{capsule}` in any browser argument becomes the capsule's own directory. You
need it: Firefox's `--screenshot` writes **nothing at all** when it is given a
relative filename (measured, and the run still exits 0), and you cannot type
the path of a capsule this command names for you. It is a token, and the engine
does not rewrite `--screenshot`, because the engine does not know which of a
browser's flags take paths, and guessing would mean a list to keep correct for
every browser and every version.

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

**The capsule directory is also the download directory**, which corrects the
first version. That version put downloads in a `downloads/` subdirectory, and
the live lab caught the cost: `--screenshot shot.png` writes `shot.png` to the
browser's working directory, which is the capsule root, so the nomination loop
looked one level down and reported that the capsule "did not produce" a file it
was standing next to. A caller would have had to know the engine's internal
layout to nominate anything the browser wrote for itself.

The profile is `.profile` and not `profile` for a functional reason: a
nomination may not start with a dot, so nobody can nominate the profile at all.
The layout guarantees it; no check has to be remembered.

### The profile is fresh, and no flag changes that

`apex browser` writes a new profile for every run. There is no `--profile`
naming one you already have, and no way to point it at `~/.mozilla`: the engine
derives the path from the capsule name, and your own profile is not in the
mount namespace to be pointed at.

That is a scope decision as much as a security one. A profile that persisted
between runs would be long-lived credential storage, a cookie jar that an
automation accumulates and that nothing expires, which is the thing capability
auth exists to replace. Persistence is listed under "what is not built" so that
adding it later has to argue for itself.

### Cookies

Cookies are a file in the profile, so they inherit the profile's lifetime: the
browser creates them inside the capsule and the engine deletes them with it.
The claim that matters is the negative one, that the capsule cannot read the
cookies of the browser you use. It holds because of the masked home, and it
would hold however careless the capsule was: no code path could copy them in.

**A cookie jar cannot be carried out either**, and that follows from the
layout, not from a rule somebody wrote. Cookies live in the profile, the
profile is `.profile`, and a nomination may not start with a dot or carry a
directory component, so no filename names the cookie jar. A capsule's session
ends with the capsule.

That is the stronger position, and this verb takes it. Handing a session back
to the host is what a persistent profile would be for, and persistence is
listed under "what is not built" for the same reason: a cookie jar an
automation accumulates and nothing expires is long-lived credential storage,
which is what capability auth exists to replace.

### Downloads

The profile pins the download directory to the capsule and turns off the "ask
me where" dialogue, because a headless browser has nobody to ask.

Downloads reach the host the same way a disposable VM's files do:

* `--download NAME` nominates a plain filename. No directory component, no
  `..`, no wildcard: a wildcard would let the browser choose what leaves by
  choosing a filename.
* `--download-to DIR` is the only thing that makes anything leave at all.
  Without it nothing does, however much is nominated.
* The copy loop runs over the nominations. It never lists the capsule and
  copies what it finds, which is the same direction `apex vm run` documents as
  its security property.
* A nominated file that would land on top of an existing one is not copied
  unless `--force` says so. The file came from a page you did not trust enough
  to open in your own browser, and replacing something of yours with it is a
  decision.
* `--download-to` is refused if it resolves inside the capsule root, because
  teardown deletes that tree and every file would be reported copied and then
  removed.

## The network

A capsule with neither `--allow` nor `--capability` is refused, for the reason
`AgentPolicy::validate_for` already gives for an allowlisted session with an
empty allowlist: a browser that can reach nothing while reporting a destination
policy is a mode that lies.

Each `--allow` destination must already be on the runtime's allowlist
(`apex agent allow <host>`), and the refusal names the exact line to add. This
narrows the set the daemon enforces; it is not a second grant path.

**The destinations a capsule names are the only ones it can reach** (P2-012,
protocol 9). They travel to the daemon as `RunRequest::allow`. The daemon
proves every line is covered by a rule the runtime's own list already carries,
and the egress proxy enforces the narrowed list, not the runtime's, which is
also what `apex agent status <id>` prints as `destinations`. Before protocol 9
the daemon snapshotted the runtime's whole allowlist for every session, so a
capsule started to visit one host could reach every destination the machine had
ever been told to permit, and the check in `apex browser` was a check in a
shell script and no boundary at all. The engine's check is still there, because
a refusal before a capsule directory exists names the line to add and costs
nothing to show, but it no longer confines the capsule.

The narrowing can only subtract. A line the runtime does not cover is refused
and not dropped: a caller quietly given less than they named finds out minutes
later, inside the capsule, as a network error with no cause attached. An empty
list is refused too. "Reach nothing" is `--network offline`, and one policy
should have one spelling.

This is what makes `--capability` a boundary. The pinned `host:port` from the
credential replaces the capsule's destinations, so the proxy confines a capsule
holding a capability to the endpoint that capability is for.

The engine points the browser at the in-namespace bridge with profile
preferences and not with `HTTP_PROXY`, because Firefox does not read the
environment variables. The engine writes `network.proxy.type=1` and the
http/ssl proxy host and port into the profile, which makes those preferences
part of the security surface, and the suite asserts them.

Ignoring them buys nothing, and the lab measures that instead of quoting it. A
capsule's interface list is `lo` and nothing else, so a connection aimed past
loopback has no route to try: `curl` to an address in TEST-NET-2 fails to
connect in 0 ms. A connection by *name* does not get that far, because there is
no resolver in the namespace at all. The daemon resolves, which is also why the
allowlist checks a name and not an address.

The enforcer supplies separate evidence that the **browser** uses the bridge,
and not only `curl`: a capsule pointed at a destination nobody allowed produces
a denial in agentd's own log naming that host, and only a proxied request could
have produced it.

Two consequences follow. They are limits, and neither is a hole:

* **The proxy tunnels `CONNECT` only.** The daemon refuses a plain `http://`
  URL with a 405, so a capsule talks to https sites.
* **A tunnel is opaque, with exactly one exception, named at the call site.**
  This is a destination policy: a capsule allowed to reach a host can send that
  host anything, and nothing between the two reads it. The exception is
  `--present` (P2-012, route B, protocol 11). For a capsule that named a
  credential, the runtime terminates that credential's ONE pinned destination
  instead of tunnelling it, so for that destination the runtime reads the
  request and `apex-secretd` reads the request and the answer. Every other
  `CONNECT` the same capsule makes is opaque as before. A test demonstrates this
  by asking which certificate the capsule was handed. A capsule that names no
  credential has no terminated destination at all.

## The browser talks to its vendor, and the allowlist stops it

A capsule's profile turns off telemetry, the updater, remote settings, the
region and geolocation services, the captive-portal and connectivity probes,
safebrowsing and prefetch. That is hygiene, and it is not enough: with all of
it applied, the lab's daemon still logs a capsule being denied
`firefox.settings.services.mozilla.com` and `aus5.mozilla.org`.

**The allowlist is the boundary; the preferences are hygiene.** A capsule
reaches exactly the destinations it was allowed, whatever the browser tries to
contact, and the daemon writes down every refusal. The preferences exist so
that a run's allowlist describes what the capsule talks to, near enough.
Nothing depends on them.

## Firefox keeps its own sandbox, and that needed a measurement

Firefox confines its own content processes with a user namespace, which means
writing `/proc/self/uid_map` from inside APEX's sandbox. Measured, not assumed:
with `/proc` inherited read-only from the host, that write fails `EROFS`, every
content process dies on `SIGSEGV`, and the run produces no output at all while
the parent exits 0.

The agent sandbox already mounts a fresh procfs (`--proc /proc`, beside
`--unshare-pid`), so the nesting works and the browser's own sandbox survives
inside APEX's. The capsule therefore has two boundaries, and the suite asserts
the fresh `/proc` is there, so removing it fails a test instead of silently
removing a layer.

## Headless, and no flag makes it otherwise

`apex browser` has no `--headed`, no `--display` and no viewer verb, the same
way `apex vm` has no `--graphics`. A test asserts the absence.

The guarantee is structural: the capsule's `/run` and `$XDG_RUNTIME_DIR` are
tmpfs, so `$WAYLAND_DISPLAY` names a socket that does not exist, and a browser
told to open a window in there cannot find a compositor. `MOZ_HEADLESS` is set
as well, as a second layer: a capsule whose `MOZ_HEADLESS` was unset still
cannot put a window on anybody's screen, because the socket is missing.

## Capability auth: what it decides, and what it does not

`--capability NAME` names a credential already in `apex-secretd`'s root-owned
store. It binds the capsule's destination to that credential's **pin**:

* the run is refused if no credential of that name is stored;
* the capsule's allowed destination is the host **and the port** the credential
  was pinned to when it was stored, not a destination typed at the call site;
* `--allow` naming any other host is refused by name, and never added, and so
  is the same host on a different port: a different port is a different
  endpoint.

`tests/browserlab/run-browserlab`'s `capability` flow exercises the pin against
a real `apex-secretd`, with the falsifying control the claim needs: an engine
that looks the capability up and then does not apply it sends the capsule
wherever the call site asked.

That flow exists because the first version could not work at all. It read the
pin out of the second column of `apex secret list`, which is `scheme://host`
and has no port in it, so against a real secret service the destination came
out as `https://intranet.example` and every capsule naming a capability was
refused. The suite did not catch it because its stub printed a bare host there:
the stub agreed with the engine and not with the CLI. The engine now reads the
pin from `apex secret list --json`, the form with the fields in it.

The capability decides where the browser may go. That is the half of §13's
model a browser can use without overstating it: the framework's rule is that a
caller does not choose where a credential's authority is spent, and here the
caller does not choose where the capsule can reach.

**The capsule is never given the credential's value.** The engine does not
write it into the profile, put it in the environment, or pass it on the command
line. `apex secret list` never prints a value, and this engine never asks for
one.

### And with `--present`, it can spend it (P2-012, route B)

`apex browser run --capability NAME --present` adds the second half: the
capsule's requests to that credential's pinned destination **carry the
credential**, and the capsule still never holds it.

The flag takes no argument, because `--capability` already names the credential
and has already made that credential's pin the only destination this capsule
can reach. `--present` asks the runtime to authenticate it.

`docs/browser-capsule-auth.md` writes out what happens, and this page
summarises it because it changes a property stated above. The daemon mints a
certificate authority and one leaf for that destination, installs the authority
in this capsule's browser the way `--trust-ca` does, and **terminates that one
`CONNECT`** instead of tunnelling it. The plaintext crosses to `apex-secretd`,
the only process on the machine that holds credentials, which adds the
`Authorization` header and opens its own validated TLS connection to the site.

**Neither the capsule nor the agent runtime ever holds the value.** The second
half is the design: `apex-agentd` runs as the user, so any value it held, an
unconfined session of that user could read. `apex-secretd`'s own note states
the property this rests on (no verb in its protocol returns a credential), and
that sentence is still true.

It needs a grant, like every other way of spending a credential:

```
apex secret grant NAME browser.present --everywhere
```

`--everywhere` and not a project, because a capsule's working directory is the
throwaway capsule tree, and a grant recorded against it would name a path that
is gone the next time a capsule runs.

In short, the route rests on two facts. A capsule's **profile is the only thing
an engine can put inside it**: the daemon builds a session's environment, which
does not inherit the caller's, and a control that tried reached nothing. And
**anything a capsule can see, the caller can take home**, because `--download`
is a path out of the capsule by design. Handing the capsule a session cookie
would therefore hand the caller the credential, which is why that route is
rejected and this one is not: the value is never in the capsule to be carried
out.

## One private CA, for one capsule (P2-012, gap 5)

```
apex browser run --allow intranet.corp:443 --trust-ca ~/corp-root.pem \
    --download page.png --download-to ~/results \
    -- --screenshot {capsule}/page.png https://intranet.corp/
```

The intranet case: a site whose certificate is signed by an organisation's own
root. Without this flag a capsule refused it. A fresh profile has a fresh
`cert9.db` and trusts the system store, and the alternative was adding the root
to the machine, permanently, for the sake of one automated run.

**It is the BROWSER's trust and not the session's**, and that matters: `curl`,
`git` and `python` in the same capsule keep using the system bundle and still
refuse the host. The daemon installs a Firefox enterprise policy, the only CA
install route the image has; `nss-tools` is not installed, so there is no
`certutil`.

**The flag names a file and has no default.** Handing a capsule a CA it did not
have widens what it will believe, and a default would widen it without anybody
typing anything.

### What happens, and what does not

The daemon (not this command, and not the capsule) reads the PEM, copies it
where the session can read and not write it, adds a `Certificates.Install` to a
copy of **the machine's own** `/etc/firefox/policies/policies.json`, and binds
that copy over the real one inside the session's mount namespace. The machine's
file is never written, and no other browser on it sees the root.

The daemon MERGES the machine's policy instead of replacing it, which is the
half a simpler implementation would get wrong while still trusting the CA. The
shipped file carries four preferences at `Status: "default"`, so a capsule
handed a document containing only a CA would be a browser with different
defaults from every other browser on the machine, for a reason nobody could
see.

Four things refuse instead of warning, because a capsule that asked to trust a
CA and did not get one fails **silently**. Firefox answers an untrusted chain
by sitting on it, so the run ends at `--timeout` with an empty screenshot and
no cause named anywhere:

* a PEM carrying anything but certificates. `cat key.pem cert.pem >
  bundle.pem` is ordinary, the copy lands where the agent can read it, and a
  trust anchor is public while the key beside it is not;
* DER instead of PEM, refused with the `openssl x509 -inform DER` line that
  converts it;
* a machine with no `/etc/firefox/policies/policies.json`. `--ro-bind-try`
  over a path that is not there is a **silent no-op**, so binding hopefully
  would produce exactly the five quiet minutes above;
* a machine whose policy already installs certificates. Merging two trust
  lists silently, or dropping somebody else's, is not a decision to take for
  them.

### How it is measured

`apexd/apex-agentd/tests/browser_ca_bind.rs` starts a private daemon and lets
the SESSION do the measuring: it copies out what it finds at
`/etc/firefox/policies/policies.json` inside its own namespace, reads the path
that document names, and copies that out too. The assertions are about bytes a
confined process produced.

The control is half of the test. A session started the same way with no
`--trust-ca` must see the machine's own file byte for byte, with no
`Certificates` key. Without that control, "the capsule sees a policy with a CA
in it" would also hold for a daemon that installed one into every session on
the machine. The test reads `/etc` and asserts its hash unchanged afterwards.

`docs/browser-capsule-auth.md` measures separately, against a real browser and
a real TLS server, that a real Firefox then *accepts* such a root. That
measurement is not in CI: it needs a fixture CA, a server and a browser for a
question that is asked once.

### The wire

`RunRequest::trust_ca`, **protocol 10**, and the CLI refuses to send it to a
daemon below that revision. It is the first guarded field on that list whose
dropped key fails CLOSED (the capsule trusts less, never more), and it has a
number anyway, because the closed failure is the silence above and nobody
would see a refusal. `apex-agent-core/src/protocol.rs` argues it at
`PROTOCOL_VERSION`.

## What is not built

* **A capsule cannot log in to a site that asks for a FORM.** `--present` puts
  a credential on a request and cannot fill in a login page or follow an OAuth
  redirect chain. For those the answer is still a driver in the capsule (the
  WebDriver bullet below), which needs a package that is not in the image.
* **The response direction is scrubbed for the credential, and that is no
  guarantee about the site.** A site that echoes the header back has it
  redacted in place; a site that transforms the value first does not. A
  credential spent at a site is a credential that site has.
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
* **Every capsule reads the machine's own Firefox enterprise policy**, because
  the sandbox binds `/` read-only, which is also why the bind above works.
  Today `/etc/firefox/policies/policies.json` carries four preferences at
  `Status: "default"` and nothing else, so it overrides nothing a capsule does.
  A `Certificates.Install` there, or a `Proxy` at `Status: "locked"`, would
  change what every capsule believes or where it connects. `Containerfile.base`
  asserts the file's shape positively (`policies` carries `Preferences` and
  nothing else, every preference at `Status: "default"`), so all three of those
  additions fail the image build instead of shipping. It is a build assertion
  and not a check in this command, because the file belongs to the image build;
  `tests/check-containerfile-assertions.sh` runs it against the repository
  first.
