# Application permissions in APEX (P1-061)

APEX already answers "who is asking, and from where" for agents: `origin::classify`
establishes one of §7's seven request origins from the kernel's view of a
connection, and `privilege::decide` reads a policy table off it. `apex-secretd`
answers the same question for credentials, with a capability record that travels
unchanged from the caller to the broker to the audit log.

This document is the second axis of that question — **what may an ordinary
application touch** — and the reason it needed its own design rather than another
row in an existing table is that, unlike an agent session, an application's answer
is not enforced by APEX. It is enforced by somebody else, or by nobody, and which
of those it is changes per capability, per application and per login session.

The whole point of this design is to say that out loud.

---

## 1. The lie this exists to prevent

The obvious Settings page has one column of application names, one column of
capability names, and a tick. It is wrong, and it is wrong in the direction that
gets somebody hurt: a tick beside "Camera" for a native binary reads as a
statement that unticking it would stop the camera, and it would not.

The second-most-obvious page fixes that by splitting the world in two —
Flatpaks are sandboxed, native applications are not — and that is also wrong,
less visibly so. Two applications installed on this machine prove it.

`io.github.cosmic_utils.camera` is a Flatpak. Its manifest context is:

```
[Context]
shared=ipc;
sockets=wayland;pulseaudio;fallback-x11;
devices=all;
filesystems=xdg-run/pipewire-0:ro;xdg-pictures;xdg-videos;xdg-config/cosmic;/run/udev:ro;
```

`devices=all` binds the host `/dev` into the sandbox. That application does not
need `org.freedesktop.portal.Camera` and will never appear in the portal
permission store, because it can open `/dev/video0` directly. Revoking its camera
permission in the store revokes nothing. Its camera access is enforced by exactly
what a native binary's is: the file mode on the device node and the ACL logind
puts there.

`com.spotify.Client` is a Flatpak with a much tighter context — `devices=dri`, no
`--device=all` — but it still carries `sockets=pulseaudio` and
`filesystems=xdg-run/pipewire-0:ro`. There is no microphone portal in
xdg-desktop-portal. Microphone access for a Flatpak is the PipeWire/PulseAudio
socket being in the sandbox, decided once at install time, and nothing brokers a
request for it at all.

So the axis is not *Flatpak versus native*. The axis is **which primitive, if
any, actually stands between this application and this capability**, and that has
to be computed per pair.

---

## 2. What was measured, and on what

Every fact in this section was read off a live APEX machine (`l16`, image
rev 48fb1b26, Hyprland session) on 2026-09-12. Nothing was written; nothing was
revoked. Package versions:

| package | version |
| --- | --- |
| `xdg-desktop-portal` | 1.20.4-1.fc43 |
| `xdg-desktop-portal-gtk` | 1.15.3-2.fc43 |
| `xdg-desktop-portal-gnome` | 49.0-1.fc43 |
| `xdg-desktop-portal-hyprland` | 1.4.1-1.fc43 |
| `xdg-desktop-portal-wlr` | 0.8.1-1.fc43 |
| `flatpak` | 1.16.6-1.fc43 |
| `pipewire` | 1.4.11-1.fc43 |

### 2.1 Which portal interfaces exist depends on which session you logged into

This is the single most surprising measured fact, and it shapes the model.

`xdg-desktop-portal` exports an interface on `org.freedesktop.portal.Desktop`
only if a *backend selected by this session's `portals.conf`* implements the
matching `org.freedesktop.impl.portal.*`. APEX ships three sessions with three
different configurations:

| session | `portals.conf` | source |
| --- | --- | --- |
| labwc | `default=gtk`, ScreenCast/Screenshot `wlr`, Secret `gnome-keyring` | `files/system/xdg-desktop-portal/labwc-portals.conf` |
| niri | `default=gtk;gnome`, ScreenCast/Screenshot `gnome`, Secret `gnome-keyring` | `files/system/xdg-desktop-portal/niri-portals.conf` |
| Hyprland | `default=hyprland;gtk` | **was the packaged file, with no APEX override — §7** |

`hyprland.portal` implements Screenshot, ScreenCast, GlobalShortcuts and
InputCapture. `gtk.portal` implements FileChooser, AppChooser, Print,
Notification, Inhibit, Access, Account, Email, DynamicLauncher and Settings.
Between them they implement neither Usb, nor RemoteDesktop, nor Secret — all
three of which `gnome.portal` does implement, and which the niri configuration
therefore reaches and the Hyprland configuration does not.

Introspecting `org.freedesktop.portal.Desktop` in the live Hyprland session
confirms it exactly. Present: Account, Camera (v1), DynamicLauncher, Email,
FileChooser, GameMode, GlobalShortcuts, Inhibit (v3), InputCapture, Location
(v1), MemoryMonitor, NetworkMonitor, Notification, OpenURI, PowerProfileMonitor,
Print, ProxyResolver, Realtime, ScreenCast (v5), Screenshot, Settings, Trash.
Absent: **`org.freedesktop.portal.Usb`**, **`org.freedesktop.portal.RemoteDesktop`**,
**`org.freedesktop.portal.Secret`**.

Two consequences the model has to carry:

* "There is no USB permission for this app" is not the same statement as "this
  app is denied USB". On this session there is no USB *portal*, so no
  application — sandboxed or not — is being brokered for device access, and a
  page that showed a USB row with a "denied" tick would be inventing an
  enforcement that is not running.
* **The Hyprland session had no Secret portal.** A real gap rather than a
  design decision: the niri and labwc configurations both pin
  `org.freedesktop.impl.portal.Secret=gnome-keyring` and Hyprland's did not,
  purely because APEX had never written an override for it.
  `gnome-keyring.portal` additionally carries `UseIn=gnome`, so it was not
  reachable by any default list in a Hyprland session either — only by being
  named. Closed on this branch; §7 has the fix and the experiment that
  established the mechanism. The measurement above is from before it, and is
  left as measured.

### 2.2 The portal permission store, and the third answer

`org.freedesktop.impl.portal.PermissionStore` is live
(`/usr/libexec/xdg-permission-store`), backed by GVariant files under
`~/.local/share/flatpak/db/`. The tables that exist on this machine:

```
notifications	notification	app.zen_browser.zen	yes	0x00
notifications	notification	org.mozilla.thunderbird	yes	0x00
desktop-used-apps	x-scheme-handler/spotify	app.zen_browser.zen	com.spotify.Client,1,3	0x00
documents	9c72627b	org.mozilla.thunderbird	read,write,grant-permissions	(...)
documents	f2268abf	app.zen_browser.zen	read,write,grant-permissions	(...)
documents	8f4439e7	org.localsend.localsend_app	read,write,grant-permissions	(...)
location	location	app.zen_browser.zen	EXACT,2215716777	0x00
```

There is **no `devices` table**. `devices`/`camera` is where the Camera portal
records a grant, and its absence means no application on this machine has ever
been asked about the camera. That is the third answer this design keeps
separate: not granted, not denied, *never asked*. Collapsing it into "denied"
would tell the owner their camera is protected when the first request will
silently pop a dialog and be approved; collapsing it into "allowed" is the lie
from §1.

There is also no `screencast` table, which is a different kind of absence:
ScreenCast persistence is opt-in per request (`persist_mode`), and an
application that never asks for a restore token gets re-prompted every session
by design. "Never asked" is the correct and permanent answer there.

### 2.3 Native applications: the ACL, and why it is not per-application

```
/dev/video0        root:video 660   →  user:andre:rw-  (POSIX ACL)
/dev/snd/pcmC2D0c  root:audio 660   →  user:andre:rw-  (POSIX ACL)
```

`id` reports `andre` in `andre`, `wheel`, `kvm` — **not** in `video` or `audio`.
The group bits are therefore not what grants access. The ACL entry is, and
systemd-logind puts it there through the `uaccess` udev tag for whoever holds
the active seat.

That ACL is attached to the *user*, not to a program. Every process running as
`andre` — a native binary, a Flatpak with `devices=all`, a shell script, an
agent — can `open("/dev/video0")`. There is no primitive in this image that
distinguishes one of them from another, and therefore:

> **A native application's camera permission cannot be revoked.** The only
> control that exists removes the capability from the entire login session at
> once, and taking it would break every application including the one the owner
> was trying to keep.

The same is true of the microphone, of the network, and of the home directory.
This is not a defect APEX can patch around at this layer; portals are opt-in and
a program that does not call one is not intercepted. Saying so precisely is the
deliverable. The roadmap's own wording — "unifies **or brokers** … **where
Linux/portal primitives permit**" — is what makes documenting the absence an
answer rather than a cop-out.

### 2.4 What WirePlumber does with the store

`/usr/share/wireplumber/wireplumber.conf` declares
`libwireplumber-module-portal-permissionstore` (`support.portal-permissionstore`)
and `client/access-portal.lua` (`script.client.access-portal`), and the virtual
`policy.client.access` *wants* the latter. Both appear under
`mixin.systemwide-session` as `disabled`, but that is a mixin the `main` user
profile does not inherit, so in a user session they are active: WirePlumber asks
the permission store whether a client may see camera nodes.

That last sentence used to be read off the config file. It is now read off the
running process: on the L16, 2026-09-12,
`grep libwireplumber-module /proc/<wireplumber>/maps` lists
`libwireplumber-module-portal-permissionstore.so` among the session
WirePlumber's mapped modules. The module is loaded on this machine, right now,
and not merely enabled on paper.

**Measured, 2026-09-12: it does NOT.** Changing `devices`/`camera` in the store
does not affect a PipeWire client that is already connected.
`tests/measure-permission-store-reach.sh` reproduces it on a private session
bus, a private PipeWire, a private WirePlumber and a private `XDG_DATA_HOME`, so
the store it writes belongs to an application id that does not exist and the
machine's own grants are never touched. Nothing here needed a camera taken away
from anybody.

The negative is only worth something because every other link was verified
working in the same run:

* The client really is a portal client. The server records
  `pipewire.access = portal`, on socket `pipewire-0`, with
  `pipewire.access.portal.app_id` and `media_roles = Camera`.
* WirePlumber really did grant it. `pw-cli get-permissions` shows `rwxm-` on
  the client's own object and on the `media.role=Camera` node, and
  `client/access-portal.lua:87` logged `setting permissions: true`.
* The store really did change. `Lookup devices camera` reads back `['no']`.
* The `Changed` signal really was on the bus. `gdbus monitor` caught
  `Changed('devices','camera',false,<byte 0x00>,{app:['no']})`.
* The module really is talking to the store. Its `lookup` calls answer, logged
  by `m-portal-permissionstore`.

Six seconds after the write, the running client still had `rwxm-` on every
object, and `access-portal.lua` logged nothing at all. It acts on `object-added`
— when a client connects — and did not act on the store change.

Where the chain breaks, between the store's `Changed` signal and the handler at
`client/access-portal.lua:123`, is **not** established. This is a measurement of
the outcome, not a diagnosis, and inventing the cause would be the kind of claim
the rest of this document exists to avoid.

So the model records camera revocation timing as `NextRequest`, which is now the
*correct* statement rather than the merely conservative one, and `Timing::Immediate`
stays unreachable — held so by `nothing_claims_a_revocation_is_immediate`.

---

## 3. The model

Four independent questions per (subject, capability) pair. They are independent
because collapsing any two of them is how the page becomes a lie.

### 3.1 Subject

`Subject::Flatpak { app_id }` or `Subject::Native { id }`. Recorded because the
owner thinks in applications, **not** because it determines enforcement. §1 is
the counterexample.

### 3.2 State — *what is the answer right now*

| variant | meaning |
| --- | --- |
| `Granted` | an affirmative record exists |
| `Denied` | a negative record exists — someone said no, explicitly |
| `NeverAsked` | no record at all; the next request will prompt |
| `NoPrimitive` | nothing in this session can express an answer to this question |

`NoPrimitive` is what USB is on a Hyprland session, and it is deliberately not
a synonym for `Denied`. `Denied` means a mechanism is refusing. `NoPrimitive`
means there is no mechanism.

### 3.3 Enforcer — *what happens if the answer is no*

This is the field the tick hides, and it is computed from the manifest and the
overrides, never assumed from the subject kind.

| variant | who enforces | example |
| --- | --- | --- |
| `PortalStore { table, id }` | xdg-desktop-portal refuses the request; the store holds the answer | Camera for a Flatpak without `devices=all`; Location; Notification |
| `DocumentPortal` | a FUSE mount exports exactly the files granted, one at a time | `documents` table entries |
| `SandboxContext { key }` | bubblewrap, at application start, from the manifest plus overrides | `sockets=pulseaudio` (microphone), `shared=network`, `filesystems=` |
| `LogindAcl { node }` | a POSIX ACL on a device node, **per user, not per application** | `/dev/video0` for a native binary, or for a Flatpak with `devices=all` |
| `Nothing` | no mechanism stands in the way | a native binary's network access |

`SandboxContext` and `LogindAcl` are both real enforcement — the kernel does
stop the program — but neither is *per-application-per-capability revocable at
runtime*, and that distinction is what the revocation field below exists for.

### 3.4 Origin — *where the current answer came from*

Criterion 3 asks for origin, and it is not `RequestOrigin`; it is provenance of
the permission itself.

`Manifest` (the application shipped with it), `UserOverride { path }`
(`~/.local/share/flatpak/overrides/<app-id>` — two live on this machine, for
`com.github.bmaron.gscrcpy` and `com.modrinth.ModrinthApp`), `SystemOverride`
(`/var/lib/flatpak/overrides/` — absent here), `StoreGrant { table }` (the owner
answered a portal dialog), `SeatAcl` (logind gave it to the session), and
`Unmediated` (nobody granted it; it was never withheld).

### 3.5 Revocation — *what `apex permissions revoke` would actually do*

Not a boolean. Five answers, and three of them are "less than you think".

| variant | meaning |
| --- | --- |
| `StoreDeny` | write `no` into the permission store — an explicit refusal, which also stops the next prompt |
| `StoreForget` | delete the entry — back to `NeverAsked`, so the owner is asked again |
| `ContextEdit { key }` | write a `flatpak override`; **takes effect at next launch**, a running instance keeps what it has |
| `Unsupported { why }` | the capability is real and the access is real and nothing can take it away from this application alone |
| `NoPrimitive` | there is no permission here to revoke |

`StoreDeny` and `StoreForget` are two different user intentions — "never" and
"ask me again" — and a page with one button for both is answering a question the
owner did not ask.

Revocation timing is its own field: `Immediate`, `NextRequest`, `NextLaunch`,
`Never`. Nothing in this implementation claims `Immediate`, for the reason in
§2.4.

---

## 4. Worked rows

The rows below are what the model produces for real applications on this
machine. They are the test fixtures, and they are the argument.

| subject | capability | state | enforcer | origin | revocation |
| --- | --- | --- | --- | --- | --- |
| `com.spotify.Client` | camera | `NoPrimitive` | `Nothing` | `Unmediated` | `NoPrimitive` — `devices=dri` only, no `/dev/video*`, and it never asked |
| `com.spotify.Client` | microphone | `Granted` | `SandboxContext{sockets=pulseaudio}` | `Manifest` | `ContextEdit` — next launch |
| `com.spotify.Client` | network | `Granted` | `SandboxContext{shared=network}` | `Manifest` | `ContextEdit` — next launch |
| `io.github.cosmic_utils.camera` | camera | `Granted` | `LogindAcl{/dev/video0}` | `Manifest` (`devices=all`) | `ContextEdit` — next launch, and **only** because dropping `devices=all` is what moves it back under the portal |
| `app.zen_browser.zen` | location | `Granted` | `PortalStore{location,location}` | `StoreGrant` | `StoreDeny` or `StoreForget` — next request |
| `app.zen_browser.zen` | notifications | `Granted` | `PortalStore{notifications,notification}` | `StoreGrant` | `StoreDeny` or `StoreForget` — next request |
| any application | usb | `NoPrimitive` | `Nothing` | `Unmediated` | `NoPrimitive` — no Usb portal in this session (§2.1) |
| native `zed` | camera | `Granted` | `LogindAcl{/dev/video0}` | `SeatAcl` | `Unsupported` — the ACL is the login session's, not this program's |
| native `zed` | network | `Granted` | `Nothing` | `Unmediated` | `Unsupported` — nothing mediates a native socket |

The fourth row is the one worth re-reading. A Flatpak and a native binary land
on the same enforcer, from opposite directions, and the page shows that instead
of hiding it behind the word "sandboxed".

---

## 5. What Settings shows

`Settings → Privacy & Permissions` renders the pair, never the tick. Every row
carries state *and* enforcer, and the enforcer decides what the control is:

* `PortalStore` → two controls, "Never" and "Ask again", because those are the
  two store operations.
* `SandboxContext` → one control, labelled with the fact that it applies at next
  launch.
* `LogindAcl` / `Nothing` → **no control at all**, and a sentence saying why.
  A disabled switch invites the owner to wonder what is wrong with their
  machine; a sentence tells them the truth, which is that nothing on this system
  can do what they are asking for.

The section ordering is by enforcer strength, not alphabetical, so that
what the machine controls sits above what it only observes.

---

## 6. What this deliberately does not do

* **It does not interpose on `/dev/video0`.** A seccomp/LSM layer that caught a
  native `open()` would be a new enforcement mechanism, with its own bypasses,
  and the roadmap asked for unification or brokering *where the primitives
  permit*. Here they do not.
* **It does not shadow the permission store.** APEX writing its own parallel
  database of intentions that the portal never reads is precisely the failure
  in §1, one layer deeper. Every write this makes goes through
  `org.freedesktop.impl.portal.PermissionStore` or `flatpak override`, which are
  the things a request is checked against.
* **It does not claim revocation is immediate**, and §2.4 now says so on a
  measurement rather than on caution: a store change does not reach a PipeWire
  client that is already running.

---

## 7. Known gap, filed here rather than fixed silently

The Hyprland session has no Secret portal (§2.1). `files/system/xdg-desktop-portal/`
carries an override for labwc and one for niri, both of which pin
`org.freedesktop.impl.portal.Secret=gnome-keyring`; the Hyprland session falls
through to the packaged `default=hyprland;gtk`, and neither of those backends
implements Secret. A Flatpak asking for the Secret portal on APEX's default
session gets nothing, where the same Flatpak on the niri session gets
gnome-keyring.

**Closed on this branch.** `files/system/xdg-desktop-portal/hyprland-portals.conf`
pins `org.freedesktop.impl.portal.Secret=gnome-keyring`, restating
`default=hyprland;gtk` (a config in `/etc` replaces the one in `/usr/share`
rather than merging with it) and making the capture pins explicit so the build
can assert them. `Containerfile.base` now asserts that all three sessions pin
Secret, enumerated rather than counted.

The mechanism was verified rather than assumed, because `gnome-keyring.portal`
carries `UseIn=gnome` and a Hyprland session is not gnome. On a private D-Bus
session with `XDG_CURRENT_DESKTOP=Hyprland`, a test backend whose `.portal` file
declared `UseIn=gnome` **was** resolved when an explicit
`org.freedesktop.impl.portal.Access=<backend>` line named it, and was **not**
resolved when the same fixture left it to `default`. Naming a backend overrides
its `UseIn`; that is what makes both this pin and niri's work.

What was deliberately **not** added is `gnome` on the end of the default list.
It would also reach Usb, RemoteDesktop, Background, Clipboard, Wallpaper and
Lockdown — which is why the niri session has them — but it starts
`xdg-desktop-portal-gnome` in a session with no GNOME shell behind it, for
interfaces whose backends talk to Mutter, and whether those answer usefully
there was not measured. An interface that exists and cannot answer is worse
than one that is honestly absent: the model reports the second as
`NoPrimitive`, and would report the first as a broker that is there.

---

## 8. Using it

`apex permissions list` prints every application, every capability, and the
enforcer beside each answer. `--json` is the same report for Settings, and adds
the session's exported portal interfaces so a consumer can see what is
brokerable at all rather than inferring it.

`apex permissions show <app>` narrows that to one application. A Flatpak is
named by its application id; anything else is `native:<name>`, and the prefix is
the whole distinction the command can make — there is no list of native
applications to enumerate, because there is nothing per-application to
enumerate it from.

`apex permissions revoke <app> <capability>` writes `no` into the portal
permission store, or writes a `flatpak override`, depending on which one
enforces the capability. `--forget` deletes the stored answer instead,
so the application is asked again next time rather than silently refused —
"never" and "ask me again" being two different intentions. `--dry-run` prints
the command it would run and when the change would take effect.

Where nothing can be revoked, `revoke` **exits non-zero and says why**, in three
different sentences for three different reasons: a native subject's permission
belongs to the login session, a Flatpak with `devices=all` cannot be brokered
for a device it opens directly, and a capability this session has no portal for
is not being refused by anything. That is the answer, not an error in producing
one — and a command that printed success for something it had not done would be
the tick from §1 in another form.

Nothing on either path needs root, and nothing raises an authentication prompt:
the permission store and the override files are the user's own.
