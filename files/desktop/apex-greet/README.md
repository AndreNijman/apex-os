# apex-greet

A Quickshell/QML **greetd** greeter for APEX-OS. It is a standalone port
of the Brain_Shell lock screen: same clock, same pill auth card, same
blurred-wallpaper backdrop and shake-on-failure feel, re-implemented so
it depends on nothing from the shell (which does not exist yet at login
time).

## Files

| File | Role |
|------|------|
| `shell.qml` | Entry point. Inlined palette (`ThemeGreet`), the fullscreen layer-shell window model, wires `GreetContext` to `GreetSurface`. |
| `GreetContext.qml` | Non-visual shared state + the greetd auth backend, edition detection, session list, last-user/last-session memory. |
| `GreetSurface.qml` | The per-output visuals: clock, wallpaper/gradient, logo, username + password pills, spinner, error shake, session picker. |
| `assets/spark-{gold,chartreuse,white}.png` | Edition spark logos (256px). |
| `greetd-config.toml` | Example `/etc/greetd/config.toml` (launches sway). |
| `sway-greet.conf` | The sway host config — hosts quickshell as its only client. |
| `labwc-greet/` | Fallback host: labwc config dir (`rc.xml`, `autostart`, `environment`) + note. |

## How it works

### Window model

The greeter runs under **`sway`** (a wlroots kiosk compositor), *not* as
a session lock. sway implements `wlr-layer-shell` natively — the same
mechanism gtkgreet uses — so the fullscreen surface is a Quickshell
`PanelWindow` anchored to all four edges, on the `Overlay` layer, with
`WlrKeyboardFocus.Exclusive`. One surface is created per output via
`Variants` over `Quickshell.screens`. (There is normally a single output;
extra outputs mirror the same shared state.)

> The host **changed from `cage` to `sway` in M1**: M0 spike A proved
> `cage` 0.2.0 does **not** expose `wlr-layer-shell` to quickshell 0.3.0, so
> the `PanelWindow` never got a surface and the greeter launched but never
> painted. See **Compositor host** below and `docs/m0-results.md` "Spike A".

### Auth flow (PamContext → Greetd)

The lock screen drove PAM directly. A greeter must instead talk to
greetd, which owns the PAM conversation. The mapping:

| Lock screen (PAM) | apex-greet (Greetd) |
|---|---|
| `pam.start()` | `Greetd.createSession(username)` |
| `onResponseRequired` → `respond(pw)` | `onAuthMessage(…, responseRequired, echoResponse)` → `Greetd.respond(echoResponse ? username : password)` |
| `onCompleted(Success)` → unlock | `onReadyToLaunch` → `Greetd.launch(argv)` (quickshell exits) |
| `onCompleted(fail)` → `fail()` | `onAuthFailure(msg)` → `fail(msg)` |
| `onError` → `fail()` | `onError(msg)` → `cancelSession()` + `fail()` |

`Greetd` is `Quickshell.Services.Greetd` (docs:
<https://quickshell.org/docs/v0.1.0/types/Quickshell.Services.Greetd/Greetd>).
Guards on `Greetd.available` and `GreetdState.Inactive` keep the
conversation well-formed. On failure greetd tears the session down, so
the next Enter starts a fresh `createSession`.

### Editions

Edition is resolved in `GreetContext` with this precedence:

1. `/etc/apex-greet/edition` (first line: `gaming`, `daily`, or `mono`) — override.
2. `/etc/os-release` `VARIANT_ID` (`gaming` / `daily`).
3. Fallback `mono` (dev box / unset).

Edition drives the spark logo **and** the accent colour:

| Edition | Logo | Accent |
|---|---|---|
| `gaming` | gold spark | `#fde047` |
| `daily` | chartreuse spark | `#d9f99d` |
| `mono` (fallback) | white spark | `#a6d0f7` (the Brain_Shell blue — so off-target it is pixel-identical to the lock screen) |

### Sessions & memory

`/usr/share/wayland-sessions/*.desktop` are parsed (`Name` + `Exec`,
field codes stripped) into a bottom-centre `‹ Session ›` picker
(mouse arrows or `Alt+Left/Right`). The username is prefilled from
`/var/lib/apex-greet/last-user` and the session preselected from
`/var/lib/apex-greet/last-session`. Both files are (re)written on a
successful launch — values passed through the environment so a hostile
username cannot inject shell — with the actual `Greetd.launch` deferred
until the write flushes. An unwritable state dir is tolerated silently.

### Wallpaper

`/usr/share/backgrounds/apex/default.jpg`, blurred (`MultiEffect`,
blurMax 48, brightness −0.30, saturation −0.10) under a 0.35 black
scrim. A missing file falls back to the vertical gradient
(`darker(background)` → accent).

## Compositor host

The greeter is a quickshell **layer-shell** surface (`PanelWindow`, Overlay
layer, exclusive keyboard). It therefore needs a host compositor that serves
`wlr-layer-shell` to quickshell.

| Host | Layer-shell? | Status | Config |
|---|---|---|---|
| **`cage` 0.2.0** | **No** (for quickshell 0.3.0) | **Abandoned** — greeter launches but never paints | — |
| **`sway` 1.11** | Yes (native) | **Primary host** | `sway-greet.conf` |
| **`labwc` 0.9.6** | Yes (native) | **Fallback host** | `labwc-greet/` |

**Why not cage.** M0 spike A found that `cage` 0.2.0 does not expose
`wlr-layer-shell` to quickshell 0.3.0 (`Failed to initialize layershell
integration`), so the `PanelWindow` never gets a surface — greetd starts,
apex-greet launches, the greetd/PAM conversation is reachable, but nothing
paints. (gtkgreet works under cage only because it falls back to an
xdg-toplevel; quickshell's `PanelWindow` does not.) `sway` and `labwc` both
implement `wlr-layer-shell` v4 natively, which is the whole reason for the
M1 swap. Full detail in `docs/m0-results.md` "Spike A".

`sway-greet.conf` is a deliberately bare, keybind-free config: it does **not**
`include /etc/sway/config.d/*`, has no bar and no app launchers, hides the
cursor when idle, disables titlebars/borders, disables Xwayland, paints a
black root as a pre-map fallback, leaves output resolution to autodetect, and
runs quickshell as its **only** client via
`exec "qs -p /usr/share/apex-greet/shell.qml; swaymsg exit"` — so sway exits
the moment quickshell quits (mirroring `cage -d`'s exit-with-child). On a
successful login greetd itself terminates the greeter and starts the user
session; the `swaymsg exit` covers the cancel/crash path.

### labwc fallback

If sway misbehaves on some hardware, swap to labwc — also native layer-shell.
Point greetd at:

```toml
command = "labwc -C /usr/share/apex-greet/labwc-greet"
```

The `labwc-greet/` config dir holds `rc.xml` (no decorations, no keybinds,
tap-to-click off), `autostart` (runs quickshell, then `labwc -e` on quit), and
`environment` (XKB layout). See `labwc-greet/README.md` for caveats (no
cursor-idle-hide equivalent).

### No-GPU render fallback (untested — HW/GL-runner verify item)

On a machine with **no working GL**, the software-render combo is:

```sh
WLR_RENDERER=pixman LIBGL_ALWAYS_SOFTWARE=1 QT_QUICK_BACKEND=software \
  sway -c /usr/share/apex-greet/sway-greet.conf
```

set in the greetd session **environment** (e.g. an `environment.d` drop-in or a
wrapper), not in the sway config. Caveat: M0 spike A found `WLR_RENDERER=pixman`
**crash-looped cage** specifically; sway's pixman renderer is more robust, but
this exact combo under sway is **untested** and must be verified on real
hardware or a GL-capable runner. (For labwc, the same three vars are listed,
commented, in `labwc-greet/environment`.)

## Integration (greetd + sway)

Install the greeter tree to `/usr/share/apex-greet/` (so
`Qt.resolvedUrl("assets/…")` resolves to
`/usr/share/apex-greet/assets/…`) and drop `greetd-config.toml` at
`/etc/greetd/config.toml`. The `sway-greet.conf` and `labwc-greet/` files ship
inside the same tree.

**Base-image dependency.** `Containerfile.base` now installs **`sway`** and
**`labwc`** (both stock Fedora 43) alongside `greetd`/`quickshell` — the
images-ci work adds these packages; this greeter tree only references them. The
abandoned `cage` package can be dropped once the sway host is HW-verified.

### Containerfile snippet

```dockerfile
# Greeter runtime — sway is the host, labwc the fallback (both native
# wlr-layer-shell); cage is abandoned (M0 spike A: no layer-shell for quickshell).
RUN dnf install -y greetd sway labwc quickshell && dnf clean all

# Greeter files (from this repo's files/desktop/apex-greet) — includes
# sway-greet.conf and labwc-greet/.
COPY files/desktop/apex-greet /usr/share/apex-greet
COPY files/branding/wallpapers/apex-wallpaper-default.jpg \
     /usr/share/backgrounds/apex/default.jpg

# greetd config — its default_session.command launches
#   sway -c /usr/share/apex-greet/sway-greet.conf
COPY files/desktop/apex-greet/greetd-config.toml /etc/greetd/config.toml

# Writable state dir, owned by the greetd session user. On Fedora the
# greetd package provisions a `greetd` user (sysusers.d) — there is no
# `greeter` user — so the dir is owned by `greetd`. tmpfiles creates it at
# boot (after systemd-sysusers has run).
RUN printf 'd /var/lib/apex-greet 0755 greetd greetd -\n' \
      > /usr/lib/tmpfiles.d/apex-greet.conf

# Make greetd the display manager
RUN systemctl enable greetd.service
```

Package names (`greetd`, `sway`, `labwc`, `quickshell`) may differ per
repo/COPR; `quickshell` must be the same major version these files were written
against (developed on Quickshell 0.3.0).

### State dir & SELinux

The greeter runs as the **`greetd`** user (Fedora's packaged greetd user),
so `/var/lib/apex-greet` must be writable by it (the tmpfiles line above).
Under SELinux the dir gets `var_lib_t`; if the greeter session is confined
and the `last-user`/`last-session` write is denied, the greeter still works —
the write just fails and prefill is skipped next boot. To keep the
memory feature, either relabel the dir for the greetd domain or ship an
allow rule; verify with `ausearch -m avc -ts recent` after first login.

## Testing

**Parse check (no compositor needed).** The layer-shell window cannot
map without a Wayland backend, but the QML still compiles:

```sh
QT_QPA_PLATFORM=offscreen qs -p files/desktop/apex-greet/shell.qml
```

A clean parse ends with a runtime error `No PanelWindow backend loaded`
at the `PanelWindow` line — QML syntax/type errors would be reported
*before* that. (Auth needs a live greetd socket, so full behaviour is
only testable in a VM.)

**Nested run on a dev box.** With sway installed, run the host config nested
inside a window on your existing Wayland session (edit the `exec` path to the
in-tree `shell.qml`, or install the tree to `/usr/share/apex-greet` first):

```sh
sway -c files/desktop/apex-greet/sway-greet.conf
```

greetd is absent, so `Greetd.available` is false and Enter shows
"greetd is not available" — but the visuals, clock, caps-lock hint,
session picker, and shake animation are all exercised. Full auth +
launch is validated in an APEX-OS VM where greetd owns the VT.

**Config syntax check (no HW).** The sway config parses cleanly under
`sway --validate`. sway 1.11's `--validate` still brings up the wlroots
backend first, so in a headless container force the software/headless path:

```sh
# in a Fedora 43 container with sway installed:
export XDG_RUNTIME_DIR=/tmp/xrt; mkdir -p "$XDG_RUNTIME_DIR"; chmod 700 "$XDG_RUNTIME_DIR"
WLR_BACKENDS=headless WLR_RENDERER=pixman \
  sway --validate -c /usr/share/apex-greet/sway-greet.conf   # exit 0 = clean
```

(`sway-greet.conf` was validated exactly this way against sway 1.11 in a
`fedora-bootc:43` container — clean, exit 0. labwc has no `--validate`; its
`rc.xml` is checked for XML well-formedness with `xmllint --noout`.)

## Differences from the Brain_Shell Lockscreen

| Aspect | Brain_Shell `Lockscreen.qml` | apex-greet |
|---|---|---|
| Window primitive | `WlSessionLock` + `WlSessionLockSurface` | `PanelWindow` (layer-shell) over `Variants`/`Quickshell.screens`, under sway (labwc fallback) |
| Auth backend | `Quickshell.Services.Pam` (`PamContext`, config `system-auth`) | `Quickshell.Services.Greetd` (`createSession`/`respond`/`launch`) |
| Unlock/exit | sets `LockState.locked = false` | `Greetd.launch(argv)` → quickshell exits, session starts |
| Palette | `Theme`/`Colors` singletons (live, from colors.json) | inlined `ThemeGreet` (colors.json fallbacks), accent per edition |
| Wallpaper | `WallpaperService.currentWall` | fixed `/usr/share/backgrounds/apex/default.jpg` |
| Username | display only (`$USER`); PAM resolves the auth user | editable field, prefilled from `last-user`, passed to `createSession` |
| Extra UI | none | edition spark logo, session picker |
| State | per-surface | shared `GreetContext` across surfaces |
| Clock, pill geometry, spinner, shake, caps heuristic, Escape-clears | — | **identical** |

## Known limitations / to verify

- **Live render under sway not yet HW-verified (the key M1 open item).**
  M1 switched the host from `cage` to `sway` (see **Compositor host**), which
  fixes the layer-shell gap that kept the greeter from painting (M0 spike A:
  `cage` 0.2.0 gave `Failed to initialize layershell integration`). The
  `sway-greet.conf` config is syntax-clean (`sway --validate`, sway 1.11), but
  **quickshell 0.3.0's `PanelWindow` actually mapping and painting under sway
  has NOT been confirmed on real pixels yet** — M0's headless QEMU could not
  present a capturable GL surface (`virtio-vga` gives no usable GL scanout;
  `egl-headless` exposes no 2D surface to `screendump`). Verify visual render +
  a full login on real hardware or a GPU/virgl-capable runner. Also unverified
  there: the no-GPU `WLR_RENDERER=pixman` fallback (it crash-looped *cage* in
  the spike; sway's pixman renderer is expected to be more robust but is
  untested) — see **No-GPU render fallback**.
- **`echoResponse` heuristic.** Hidden prompts get the password, visible
  prompts get the username. Correct for the usual `pam_unix` password
  prompt after `createSession`; a PAM stack that asks something else
  (e.g. an OTP) would need extra handling.
- **`launch` wrapping.** The session `Exec` is run as `sh -lc "<Exec>"`
  for PATH/profile resolution. Some sessions may prefer the raw argv or
  a `dbus-run-session`/`uwsm` wrapper — adjust `launch()` if so.
- **Empty password** is not submitted (parity with the lock screen), so
  passwordless accounts are not handled.
- Developed against Quickshell **0.3.0** / greetd docs **v0.1.0**; the
  `Greetd` API should be re-checked against the version actually shipped.
