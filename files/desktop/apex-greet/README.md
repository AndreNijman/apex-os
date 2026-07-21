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
| `greetd-config.toml` | Example `/etc/greetd/config.toml`. |

## How it works

### Window model

The greeter runs under **`cage`** (a kiosk Wayland compositor), *not* as
a session lock. cage implements `wlr-layer-shell` — the same mechanism
gtkgreet uses — so the fullscreen surface is a Quickshell `PanelWindow`
anchored to all four edges, on the `Overlay` layer, with
`WlrKeyboardFocus.Exclusive`. One surface is created per output via
`Variants` over `Quickshell.screens`. (Under cage there is normally a
single output; extra outputs mirror the same shared state.)

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

## Integration (greetd + cage)

Install the greeter tree to `/usr/share/apex-greet/` (so
`Qt.resolvedUrl("assets/…")` resolves to
`/usr/share/apex-greet/assets/…`) and drop `greetd-config.toml` at
`/etc/greetd/config.toml`.

### Containerfile snippet

```dockerfile
# Greeter runtime
RUN dnf install -y greetd cage quickshell && dnf clean all

# Greeter files (from this repo's files/desktop/apex-greet)
COPY files/desktop/apex-greet /usr/share/apex-greet
COPY files/branding/wallpapers/apex-wallpaper-default.jpg \
     /usr/share/backgrounds/apex/default.jpg

# greetd config
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

Package names (`greetd`, `cage`, `quickshell`) may differ per repo/COPR;
`quickshell` must be the same major version these files were written
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

**Nested run on a dev box.** With cage installed, run the greeter inside
a nested window on your existing Wayland session:

```sh
cage -- qs -p files/desktop/apex-greet/shell.qml
```

greetd is absent, so `Greetd.available` is false and Enter shows
"greetd is not available" — but the visuals, clock, caps-lock hint,
session picker, and shake animation are all exercised. Full auth +
launch is validated in an APEX-OS VM where greetd owns the VT.

## Differences from the Brain_Shell Lockscreen

| Aspect | Brain_Shell `Lockscreen.qml` | apex-greet |
|---|---|---|
| Window primitive | `WlSessionLock` + `WlSessionLockSurface` | `PanelWindow` (layer-shell) over `Variants`/`Quickshell.screens`, under cage |
| Auth backend | `Quickshell.Services.Pam` (`PamContext`, config `system-auth`) | `Quickshell.Services.Greetd` (`createSession`/`respond`/`launch`) |
| Unlock/exit | sets `LockState.locked = false` | `Greetd.launch(argv)` → quickshell exits, session starts |
| Palette | `Theme`/`Colors` singletons (live, from colors.json) | inlined `ThemeGreet` (colors.json fallbacks), accent per edition |
| Wallpaper | `WallpaperService.currentWall` | fixed `/usr/share/backgrounds/apex/default.jpg` |
| Username | display only (`$USER`); PAM resolves the auth user | editable field, prefilled from `last-user`, passed to `createSession` |
| Extra UI | none | edition spark logo, session picker |
| State | per-surface | shared `GreetContext` across surfaces |
| Clock, pill geometry, spinner, shake, caps heuristic, Escape-clears | — | **identical** |

## Known limitations / to verify

- **Compositor host: cage is insufficient — use sway or labwc.** M0 spike A
  found that `cage` 0.2.0 does not expose `wlr-layer-shell` to quickshell
  0.3.0 (`Failed to initialize layershell integration`), so the greeter's
  `PanelWindow` never gets a surface and nothing paints — even though
  greetd starts, apex-greet launches, and the greetd/PAM conversation is
  reachable. Switch `default_session.command` to a compositor that
  provides layer-shell: `sway` (1.11) or `labwc` (0.9.6), both packaged,
  e.g. `sway -c /usr/share/apex-greet/sway-greet.conf` running quickshell
  as its only client. Live render + login must be re-verified on real
  hardware or a GPU/virgl-capable runner (headless QEMU with `virtio-vga`
  gives no GL; `WLR_RENDERER=pixman` crash-looped cage in the spike).
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
