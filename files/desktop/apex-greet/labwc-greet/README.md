# labwc-greet — fallback compositor host

A documented **second option** for hosting the apex-greet greeter, for the
case where `sway` misbehaves on some hardware. Like sway, **labwc implements
wlr-layer-shell natively**, so apex-greet's `PanelWindow` (Overlay layer,
exclusive keyboard) maps here — which it does NOT under `cage` 0.2.0 (M0 spike
A; see `docs/m0-results.md`). sway (`../sway-greet.conf`) remains the primary
host; reach for labwc only if sway has a driver/output issue on a given box.

## Files

| File | Role |
|------|------|
| `rc.xml` | Minimal labwc config: no decorations, no keybinds, tap-to-click off. |
| `autostart` | Launches `qs` as the sole client, then exits labwc when it quits. |
| `environment` | XKB layout (+ commented no-GPU render fallback vars). |

labwc reads a config **directory**, so it is launched as:

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

(The shipped `greetd-config.toml` uses sway; this is the swap-in line.)

## Known caveats vs the sway host

- **Cursor-idle-hide:** labwc has no rc.xml equivalent of sway's
  `hide_cursor`. Cosmetic only — the greeter is keyboard-driven and its own
  opaque surface fills the screen.
- **Exit-on-quit:** `autostart` uses `labwc -e` to bring the compositor down
  if quickshell exits without a session handoff (cancel/crash). On a
  successful login greetd terminates the greeter itself, so this is only the
  fallback path. `labwc -e` needs a labwc build that supports `-e`
  (0.9.6 does); a `pkill` fallback is included.
- **Untested on real HW** — same render-verify caveat as sway (see parent
  README "Known limitations / to verify").
