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
| ignoring the proxy opens nothing | the same `--unshare-net`: no route, no resolver, so an unproxied connection is `ENETUNREACH`, not a direct one |
| downloads leave only when nominated | `apex vm run`'s egress rule — the loop runs over the **nominations**, never over the directory's contents |
| nothing survives the run | `apex vm run`'s four-way fence on a recursive removal: narrow name pattern, final component not a symlink, `realpath`, and the resolved path equal to exactly `<root>/<name>` |

So `apex browser` is a client of `apex agent`, not a second sandbox. It builds
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

## The capsule

```
apex browser run --allow example.com:443 \
    --download page.png --download-to /home/u/results \
    -- --screenshot page.png https://example.com/
```

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

A cookie jar can be carried **out** like any other file, by nominating it. That
is deliberate: a caller who wants the session the automation established has to
name it, and it lands under the same no-clobber rule as everything else.

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

Ignoring them buys nothing. The namespace has no route and no resolver, so a
browser that bypassed the proxy would get `ENETUNREACH` rather than a direct
connection.

Two consequences, stated because they are limits rather than holes:

* **The proxy tunnels `CONNECT` only.** A plain `http://` URL is refused by the
  daemon with a 405, so a capsule talks to https sites.
* **A tunnel is opaque.** This is a destination policy: a capsule allowed to
  reach a host can send that host anything.

## Firefox keeps its own sandbox, and that needed a measurement

Firefox confines its own content processes with a user namespace, which means
writing `/proc/self/uid_map` from inside APEX's sandbox. Measured rather than
assumed: with `/proc` inherited read-only from the host, that write fails
`EROFS`, every content process dies on `SIGSEGV`, and the run produces no
output at all while the parent exits 0 — a silent nothing.

The agent sandbox already mounts a fresh procfs (`--proc /proc`, beside
`--unshare-pid`), so the nesting works and the browser's own sandbox survives
inside APEX's. The capsule therefore has two boundaries, not one, and the
suite asserts the fresh `/proc` is there so that removing it fails a test
instead of silently removing a layer.

## Headless, and there is no flag to make it otherwise

`apex browser` has no `--headed`, no `--display` and no viewer verb, the same
way `apex vm` has no `--graphics`. The absence is asserted rather than
reviewed.

More importantly the guarantee is structural: the capsule's `/run` and
`$XDG_RUNTIME_DIR` are tmpfs, so `$WAYLAND_DISPLAY` names a socket that does
not exist. A browser told to open a window in there cannot find a compositor.
`MOZ_HEADLESS` is set as well, but it is the belt and the missing socket is the
braces — a capsule whose `MOZ_HEADLESS` was unset still cannot put a window on
anybody's screen.

## Capability auth: what it decides, and what it does not

`--capability NAME` names a credential already in `apex-secretd`'s root-owned
store. What it does is bind the capsule's destination to that credential's
**pin**:

* the run is refused if no credential of that name is stored;
* the capsule's allowed destination is the host the credential was pinned to
  when it was stored, not a host typed at the call site;
* `--allow` naming any other host is refused, by name, rather than added.

So the capability decides where the browser may go. That is the half of
§13's model a browser can honestly use: the framework's load-bearing rule is
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
therefore not built. It needs either a driver in the capsule that can be handed
a token for one request, or a provider that performs the login and hands back a
session — both of which are more than a flag.

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
