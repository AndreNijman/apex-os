# Verifying Rime Floating (labwc)

The roadmap's labwc test matrix, split into what a script can check and what
needs a person.

**Automated:** `tests/test-labwc-session.sh` runs real clients against a real
nested labwc using the shipped config. 18 checks. Run it before you touch
anything in `files/desktop/labwc/`.

**Also automated, elsewhere:** `tests/test-rime-firstrun.sh` covers config
seeding, XML validity, the window-chrome theme keys and keybind parity against
the shell's own defaults.

**Manual:** the rest of this page. It needs real outputs, real windows and a
person looking at the screen. Nothing in this repo claims any of it as done. If
you have not walked a checklist, treat it as unwalked.

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

The automated test proves the config names installed backends. It cannot prove
that a video call gets a picture. Log into **Rime Floating** (the session
picker's name for labwc) and check:

- [ ] **Firefox:** screen share in a Jitsi/Meet call. Whole screen, then a
      single window.
- [ ] **Chromium:** the same. Chromium and Firefox use different capture paths
      often enough that one working says nothing about the other.
- [ ] **Discord (Electron):** screen share in a voice channel. A portal
      misconfiguration usually breaks Electron apps first.
- [ ] **OBS:** add a Screen Capture (PipeWire) source; confirm it previews and
      records.
- [ ] **Flatpak file chooser:** "Open File" from a Flatpak app. You should get
      the GTK chooser, not the GNOME one. The packaged `default=wlr;*` left this
      interface to chance.
- [ ] **Open URI:** click a link inside a Flatpak app; it should reach Firefox.
- [ ] **Screenshots:** the bound screenshot keys, and `grim`/`grimblast` run by
      hand.

If screen sharing fails, check the running backend first:

```
systemctl --user status xdg-desktop-portal xdg-desktop-portal-wlr
echo "$XDG_CURRENT_DESKTOP"     # must be labwc:wlroots
```

---

## Application grid

Launch each one. Confirm it opens, draws its own decorations or takes Rime's
without glitches, resizes, and closes.

- [ ] Firefox
- [ ] Chromium
- [ ] Steam, including the client's own window chrome
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

Watch for missing titlebars, wrong window sizes on open, blurry XWayland
scaling, and windows you cannot drag.

---

## Window behaviour

- [ ] Workspace switching, and that focus lands on the window you expect
- [ ] Fullscreen, maximise, minimise, restore
- [ ] Several windows of the same application
- [ ] The thumbnail window switcher (`alt-tab`): previews render, and the OSD
      appears only on the focused output
- [ ] Desktop right-click opens the Rime context menu, not the Openbox one
- [ ] With the shell killed, a plain right-click **restarts it** and then opens
      the Rime menu, which `rime-desktop-menu` tries before giving up
- [ ] With the shell killed and unable to start (e.g. rename
      `/usr/libexec/rime-shell-autostart`), a plain right-click shows a
      notification naming the fallback, and **SUPER+right-click** opens the
      `menu.xml` emergency menu

  A plain right-click does *not* fall back to `menu.xml` on its own. A labwc
  mousebind is a fixed action and cannot branch, so the fallback sits on a
  modifier instead of replacing the Rime menu without a word.

---

## Multi-monitor

Needs a second display.

- [ ] Hotplug: plug and unplug while logged in; windows should not vanish
- [ ] Arrangement, resolution and refresh via Rime Settings → Display
- [ ] Per-output scale, including a fractional value
- [ ] Rotation
- [ ] VRR, if the panel supports it
- [ ] The arrangement survives a logout and a reboot (kanshi profile)
- [ ] Identify Displays shows the right number on the right screen
- [ ] The bar and layer-shell surfaces reserve the right space on both outputs

---

## Night light, idle and lock

You cannot probe gamma in a nested session: a nested backend reports zero
gamma-capable outputs whatever the compositor supports.

- [ ] Night light warms the screen and returns to normal when disabled
- [ ] The screen locks on idle
- [ ] Unlock works, including after a suspend/resume cycle
- [ ] `rime shell lock` locks at once

---

## Recording a run

After you walk a checklist, record the result in `rimelogs/` with the image
digest and the date. Nobody can tell an unrecorded pass from a checklist nobody
walked.
