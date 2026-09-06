# P0-025: Hyprland hyprlang to Lua migration

Design note written from the API the shipped Hyprland exposes rather than from
the wiki. Hand this to the implementing agent.

## Why this is urgent

Hyprland 0.55 deprecated hyprlang for Lua. Upstream said the old syntax survives
"1-2 releases starting from 0.55", and new config features stop landing in
hyprlang. APEX ships **0.56.2** (`hyprland-0.56.2-2.fc43.x86_64`, built
2026-08-05). One more release and the config APEX seeds into each user's home
stops loading.

Source: <https://hypr.land/news/26_lua/> and
<https://wiki.hypr.land/Configuring/Start/>.

## The authoritative API is on disk, not on the web

The Hyprland package ships both of these, matching the version APEX ships:

- `/usr/share/hypr/hyprland.lua`: upstream's reference config, 357 lines.
- `/usr/share/hypr/stubs/hl.meta.lua`: the typed API stub, 1777 lines.

Read those two rather than copying wiki examples. The roadmap says the same
thing: do not freeze copied examples if upstream has changed the Lua API.

## API surface APEX needs

```lua
hl.monitor({ output = "", mode = "preferred", position = "auto", scale = "auto" })

hl.config({
    general    = { gaps_in = 5, border_size = 2, layout = "dwindle",
                   col = { active_border = { colors = {"rgba(33ccffee)"}, angle = 45 } } },
    decoration = { rounding = 10, shadow = { enabled = true }, blur = { enabled = true } },
    input      = { kb_layout = "us", follow_mouse = 1, touchpad = { natural_scroll = false } },
    misc       = { disable_hyprland_logo = false },
})

hl.env("XCURSOR_SIZE", "24")

hl.bind("SUPER + Q", hl.dsp.exec_cmd("alacritty"))
hl.bind("SUPER + mouse:272", hl.dsp.window.drag(), { mouse = true })
hl.bind("XF86AudioRaiseVolume", hl.dsp.exec_cmd("..."), { locked = true, repeating = true })

hl.device({ name = "epic-mouse-v1", sensitivity = -0.5 })

hl.window_rule({ name = "move-x", match = { class = "foo" }, float = true })
hl.layer_rule({ name = "no-anim", match = { namespace = "^bar$" }, no_anim = true })
hl.workspace_rule({ workspace = "w[tv1]", gaps_out = 0 })

hl.curve("easeOutQuint", { type = "bezier", points = { {0.23, 1}, {0.32, 1} } })
hl.animation({ leaf = "windows", enabled = true, speed = 4.79, spring = "easy" })

hl.gesture({ fingers = 3, direction = "horizontal", action = "workspace" })
hl.permission("/usr/bin/grim", "screencopy", "allow")

hl.on("hyprland.start", function() hl.exec_cmd("awww-daemon") end)
```

Two details that change how APEX generates config:

**`exec-once` becomes an event handler.** There is no `exec-once` keyword. Use
`hl.on("hyprland.start", function() ... end)` with `hl.exec_cmd` inside.

**Binds and rules return handles.** `hl.bind(...)` and `hl.window_rule(...)`
return an object with `:set_enabled(false)`. That is a better mechanism than
the current generator's unbind-then-rebind dance, which exists only because
hyprlang had no way to disable a static default.

## `hl.device` covers the controls P0-019 needs

`HL.DeviceSpec` in the stub accepts, per named device: `natural_scroll`,
`sensitivity`, `accel_profile`, `tap_to_click`, `tap_and_drag`, `drag_lock`,
`drag_3fg`, `disable_while_typing`, `left_handed`, `middle_button_emulation`,
`clickfinger_behavior`, `scroll_method`, `scroll_factor`, `scroll_button`,
`scroll_button_lock`, `scroll_points`, `tap_button_map`, `rotation`,
`transform`, `flip_x`, `flip_y`, `region_position`, `region_size`,
`active_area_position`, `active_area_size`, `absolute_region_position`,
`relative_input`, `output`, `enabled`, `keybinds`, `repeat_rate`,
`repeat_delay`, and the `kb_*` family.

P0-019 asks for natural scroll, pointer speed, acceleration profile, tap,
tap-drag, disable-while-typing, left-handed, and scroll method/factor. The stub
carries all of them, per device, so P0-019's "no fake/no-op toggles remain"
criterion is reachable on Hyprland without inventing a mechanism. Land P0-025
first and P0-019 writes Lua once instead of migrating twice.

## What APEX has to migrate

### apex-os

| Path | What it is |
|---|---|
| `files/desktop/hypr/hyprland.conf` | 298 lines, seeded per user with `@HOME@` and `@KB_LAYOUT@` substituted at first run |
| `Containerfile.base` ~1179-1221 | Copies the file and asserts on its content, including three `source = ` lines |
| `apexd/apexd-core/src/recover.rs` | Knows the generated paths for recovery |
| `apexd/apex/src/recover.rs` | Same, around line 1594 |
| `tests/test-apex-input.sh` | Asserts on `~/.config/hypr/apex-input.conf` |
| `tests/test-apex-display.sh` | Asserts on `~/.config/hypr/apex-display.conf` |
| `tests/test-apex-recover.sh` | Asserts on all three generated files |

### apex-shell

| Path | What it writes |
|---|---|
| `src/services/config_tab/KeybindService.qml` | `~/.config/apex-shell/ApexShellKeybinds.conf` |
| `src/services/config_tab/DisplayService.qml` | `~/.config/hypr/apex-display.conf` |
| input settings page | `~/.config/hypr/apex-input.conf` |

The current chain is the legacy shape the roadmap describes: a `hyprland.conf`
with `source = @HOME@/.config/hypr/apex-input.conf`, `apex-display.conf` and
`ApexShellKeybinds.conf`.

## Target layout

```
~/.config/hypr/
├── hyprland.lua          user-owned, requires the modules below
└── apex/
    ├── monitors.lua      generated by DisplayService
    ├── input.lua         generated by the input settings page
    ├── keybindings.lua   generated by KeybindService
    ├── rules.lua
    ├── appearance.lua
    ├── workspaces.lua
    └── autostart.lua
```

Lua `require` resolves against `package.path`, which will not include
`~/.config/hypr/apex/` by default. Set `package.path` at the top of
`hyprland.lua`, or use `dofile` with an absolute path. Work out which one
Hyprland's Lua state supports before designing around it, and record the
answer.

## Migration rules

ROADMAP.md section 50 sets these, quoted:

> 1. Detect an APEX-managed legacy Hyprland configuration.
> 2. Back it up with a timestamp.
> 3. Convert APEX-owned/generated state to the Lua module layout.
> 4. Migrate recognized user overrides.
> 5. Report any legacy directive that cannot be converted safely.
> 6. Never silently discard custom keybinds, rules, monitor settings, or input settings.
> 7. Make the migration idempotent.

Point 6 is the one that costs a real user their config. Andre's own
`hyprland.conf` tells the reader "it is your file now, edits here persist", so
the config invites users to edit it. An unrecognised directive must survive the
migration or get reported.

## Validation

`hyprctl reload` then `hyprctl configerrors` must be clean after any generated
change. CI must reject active legacy compositor `.conf` generation.

`hyprctl configerrors` needs a running Hyprland, so CI needs a nested headless
instance. `tests/run-nested-labwc.sh` is the existing precedent for
nested compositor testing in apex-shell.

## Out of scope

`hypridle`, `hyprlock` and the other `hypr*` tools keep hyprlang. Upstream said
so, and `src/config/hyprlock.conf` should be left alone. Only the compositor
config moves.

## Sequencing

P0-025 touches `DisplayService.qml` and `KeybindService.qml`, which P0-018 and
P0-019 also touch. Land P0-018 first, then P0-025, then P0-019 on the Lua base.
