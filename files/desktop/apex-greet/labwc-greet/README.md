# labwc-greet: fallback compositor host

A documented **second option** for hosting the apex-greet greeter, for a
machine where `sway` misbehaves. Like sway, **labwc implements wlr-layer-shell
natively**, so apex-greet's `PanelWindow` (Overlay layer, exclusive keyboard)
maps here. It does NOT map under `cage` 0.2.0 (M0 spike A; see
`docs/m0-results.md`). sway (`../sway-greet.conf`) remains the primary host;
reach for labwc only if sway has a driver or output problem on a given box.

## Files

| File | Role |
|------|------|
| `rc.xml` | Minimal labwc config: no decorations, tap-to-click off, one keybind (`W-A-s`, the screen reader). |
| `autostart` | Launches `qs` as the sole client, then exits labwc when it quits. |
| `environment` | Sets no keyboard layout on purpose (labwc inherits `XKB_DEFAULT_LAYOUT` from the APEX keymap generator); holds the commented no-GPU render fallback vars. |

labwc reads a config **directory**, so you launch it as:

```sh
labwc -C /usr/share/apex-greet/labwc-greet
```

## To use labwc instead of sway

Point greetd at labwc in `/etc/greetd/config.toml`:

```toml
[default_session]
command = "labwc -C /usr/share/apex-greet/labwc-greet"
user = "greetd"
```

The shipped `greetd-config.toml` uses sway, and its commented swap-in line
runs labwc through the wrapper:
`/usr/libexec/apex-greet-session labwc -C /usr/share/apex-greet/labwc-greet`.
Keep the wrapper: without it the greet session has no D-Bus session bus and no
screen reader can read the login screen.

## Known caveats vs the sway host

- **Cursor-idle-hide:** labwc has no rc.xml equivalent of sway's
  `hide_cursor`. This is cosmetic: the greeter is keyboard-driven and its own
  opaque surface fills the screen.
- **Keyboard layout switch:** the greeter reads and switches layouts through
  `swaymsg`, which gets no answer under labwc. The layout pill shows
  `XKB_DEFAULT_LAYOUT` and offers no switch.
- **Exit-on-quit:** `autostart` uses `labwc -e` to bring the compositor down
  if quickshell exits without a session handoff (cancel/crash). On a
  successful login greetd terminates the greeter itself, so this covers only
  the fallback path. `labwc -e` needs a labwc build that supports `-e`
  (0.9.6 does); the script falls back to `pkill`.
- **Untested on real HW:** unlike the sway host, this fallback has no recorded
  run on real hardware (see the parent README, "Known limitations / to
  verify").
