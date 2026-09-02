# Verifying APEX Floating (labwc)

The roadmap's labwc test matrix, split by what can be automated and what cannot.

**Automated:** `tests/test-labwc-session.sh` — runs real clients against a real
nested labwc using the shipped config. 18 checks. Run it before touching
anything in `files/desktop/labwc/`.

**Also automated, elsewhere:** `tests/test-apex-firstrun.sh` covers config
seeding, XML validity, the window-chrome theme keys and keybind parity against
the shell's own defaults.

**Manual:** everything below. It needs real outputs, real windows and a person
looking at the screen. None of it is claimed as done anywhere in this repo — if
you have not walked it, it has not been walked.

---

## Status

| area | state |
|---|---|
| Session capabilities (layer-shell, session-lock, idle, clipboard, screencopy, output management) | automated, passing |
| Config reload and invalid-config recovery | automated, passing |
| Portal backend selection | automated, passing |
| Window chrome, theme keys, keybind parity | automated, passing |
| Screen sharing in real applications | **manual, not yet walked** |
| Application grid | **manual, not yet walked** |
| Multi-monitor behaviour | **manual, not yet walked** |
| Night light on real hardware | **manual, not yet walked** |

---

## Screen sharing and portals

The automated test proves the config names installed backends. It cannot prove a
video call actually gets a picture. Log into **labwc (APEX)** and check:

- [ ] **Firefox** — screen share in a Jitsi/Meet call. Whole screen, then a
      single window.
- [ ] **Chromium** — the same. Chromium and Firefox use different capture paths
      often enough that one working says nothing about the other.
- [ ] **Discord (Electron)** — screen share in a voice channel. Electron apps
      are the usual first casualty of a portal misconfiguration.
- [ ] **OBS** — add a Screen Capture (PipeWire) source; confirm it previews and
      records.
- [ ] **Flatpak file chooser** — "Open File" from a Flatpak app. Should be the
      GTK chooser, not the GNOME one. This is the interface the packaged
      `default=wlr;*` left to chance.
- [ ] **Open URI** — click a link inside a Flatpak app; it should reach Firefox.
- [ ] **Screenshots** — the bound screenshot keys, and `grim`/`grimblast`
      directly.

If screen sharing fails, check the running backend first:

```
systemctl --user status xdg-desktop-portal xdg-desktop-portal-wlr
echo "$XDG_CURRENT_DESKTOP"     # must be labwc:wlroots
```

---

## Application grid

Launch each, and confirm it opens, draws its own decorations or takes APEX's
cleanly, resizes, and closes.

- [ ] Firefox
- [ ] Chromium
- [ ] Steam — including the client's own window chrome
- [ ] gamescope
- [ ] A Steam game, windowed and fullscreen
- [ ] VS Code
- [ ] A JetBrains IDE (Java/XWayland behaviour differs from Electron)
- [ ] LibreOffice
- [ ] Blender
- [ ] Discord
- [ ] A plain Qt application (`qt6ct` will do)
- [ ] A plain GTK application (Thunar)
- [ ] Wine, and an XWayland game

Watch for: missing titlebars, wrong window sizes on open, XWayland scaling
blur, and windows that cannot be dragged.

---

## Window behaviour

- [ ] Workspace switching, and that focus follows sensibly
- [ ] Fullscreen, maximise, minimise, restore
- [ ] Several windows of the same application
- [ ] The thumbnail window switcher (`alt-tab`) — previews render, and the OSD
      appears only on the focused output
- [ ] Desktop right-click opens the APEX context menu, not the Openbox one
- [ ] With the shell killed, a plain right-click **restarts it** and then opens
      the APEX menu — that is what `apex-desktop-menu` does before giving up
- [ ] With the shell killed and unable to start (e.g. rename
      `/usr/libexec/apex-shell-autostart`), a plain right-click shows a
      notification naming the fallback, and **SUPER+right-click** opens the
      `menu.xml` emergency menu

  A plain right-click does *not* fall back to `menu.xml` on its own. A labwc
  mousebind is a fixed action and cannot branch, so the fallback is on a
  modifier rather than silently substituted.

---

## Multi-monitor

Needs a second display.

- [ ] Hotplug: plug and unplug while logged in; windows should not vanish
- [ ] Arrangement, resolution and refresh via APEX Settings → Display
- [ ] Per-output scale, including a fractional value
- [ ] Rotation
- [ ] VRR, if the panel supports it
- [ ] The arrangement survives a logout and a reboot (kanshi profile)
- [ ] Identify Displays shows the right number on the right screen
- [ ] The bar and layer-shell surfaces reserve space correctly on both outputs

---

## Night light, idle and lock

Gamma cannot be probed in a nested session — a nested backend reports zero
gamma-capable outputs whatever the compositor supports.

- [ ] Night light warms the screen and returns to normal when disabled
- [ ] The screen locks on idle
- [ ] Unlock works, including after a suspend/resume cycle
- [ ] `apex shell lock` locks immediately

---

## Recording a run

When you walk this, record the result in `apexlogs/` with the image digest and
the date. An unrecorded pass is indistinguishable from one that never happened.
