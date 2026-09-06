-- APEX keybinds. Loaded by ~/.config/hypr/hyprland.lua.
--
-- APEX-owned: `apex update` replaces this file.
--
-- APEX Shell popup binds (launcher, dashboard, notifications, wallpaper,
-- clipboard, audio, network, power menu, screen-record) are owned by the SHELL,
-- not here. It generates apex/shell-keybinds.lua from
-- ~/.config/apex-shell/src/user_data/keybinds.json and that module loads AFTER
-- this one. Binding them here too caused double-fires (SUPER+Q ran both
-- killactive AND the launcher).
--
-- ── How the shell overrides one of these ─────────────────────────────────────
-- Every bind below is registered in `M.binds` under a canonical combo key, and
-- `hl.bind` returns a handle with `:set_enabled(false)`. So a user who rebinds
-- SUPER+Q in APEX Settings gets the generated module calling
--
--     require("apex.keybindings").disable("SUPER", "Q")
--
-- before it binds its own. The hyprlang generator had to emit `unbind =` lines
-- for every APEX default on every write, whether or not the user had touched
-- them, because hyprlang had no way to disable a static bind — it could only
-- remove one by key and hope nothing else wanted it. Disabling by handle
-- touches exactly the bind that is being replaced.

local M = { binds = {} }

--- Split a modifier string on whitespace and `+` alike, uppercased.
--- The shell's keybinds.json spells combos "SUPER + SHIFT", the hyprlang config
--- spelled them "SUPER SHIFT", and both have to mean the same thing.
local function split_mods(mods)
    local parts = {}
    for word in tostring(mods or ""):upper():gmatch("[^%s+]+") do
        parts[#parts + 1] = word
    end
    return parts
end

--- Canonical key for a combo: modifiers uppercased and sorted, then the key.
--- "SUPER + SHIFT", "Q" and "shift super", "q" both give "SHIFT+SUPER|Q".
--- Sorted because the generated module writes modifiers in whatever order the
--- user's keybinds.json lists them, and "SUPER SHIFT" must match "SHIFT SUPER".
function M.key(mods, key)
    local parts = split_mods(mods)
    table.sort(parts)
    return table.concat(parts, "+") .. "|" .. tostring(key or ""):upper()
end

--- Turn off the APEX default bound to this combo. Returns true if there was one.
---
--- `:remove()` rather than `:set_enabled(false)`. Both stop the bind firing, and
--- both are handle methods — the point of a handle is that it addresses THIS
--- bind rather than "whatever is on this key", which is all `unbind` could do.
--- The difference is what the rest of the system sees: a disabled bind is still
--- listed by `hyprctl binds`, with no field saying it is inert, and APEX Shell
--- reads that list to warn about conflicting shortcuts. A ghost there makes
--- that list lie about what the keyboard actually does.
---
--- Nothing is lost by removing: `hyprctl reload` re-runs this whole file, so
--- every default comes back and is re-disabled by whatever claimed it.
function M.disable(mods, key)
    local id = M.key(mods, key)
    local handle = M.binds[id]
    if not handle then return false end
    handle:remove()
    M.binds[id] = nil
    return true
end

-- Registers the bind and remembers its handle. `mods` and `key` stay separate
-- so the canonical key can be computed from the same values Hyprland is given.
--
-- Every element of the combo is joined with " + ". hl.bind parses ONLY that
-- separator: passing "SUPER SHIFT + SPACE" fails with `Unknown keysym: "SUPER
-- SHIFT", did you forget a +?`, and it fails per bind rather than raising, so
-- the mistake shows up as a `hyprctl configerrors` entry and a dead shortcut
-- instead of a broken config.
local function bind(mods, key, dispatcher, opts)
    local parts = split_mods(mods)
    parts[#parts + 1] = key
    local handle = hl.bind(table.concat(parts, " + "), dispatcher, opts)
    M.binds[M.key(mods, key)] = handle
    return handle
end

local mod         = "SUPER"
local terminal    = "alacritty"
-- Not a browser name. `apex-open-browser` opens whatever the user has set as
-- their default, so installing another browser and making it default changes
-- this key too. It used to say `firefox`, which meant SUPER+W contradicted the
-- user's own setting the moment a second browser existed.
local browser     = "/usr/libexec/apex-open-browser"
local fileManager = "thunar"
local screenshot  = "bash /usr/share/apex-shell/src/scripts/screenshot.sh"

-- ── Apps and window management ───────────────────────────────────────────────
bind(mod, "T", hl.dsp.exec_cmd(terminal),    { description = "Terminal" })
bind(mod, "Q", hl.dsp.window.close(),        { description = "Close window" })
bind(mod, "W", hl.dsp.exec_cmd(browser),     { description = "Browser" })
bind(mod, "E", hl.dsp.exec_cmd(fileManager), { description = "File manager" })

-- Lock. Not an APEX Shell default keybind, so it is bound here.
bind(mod, "L", hl.dsp.exec_cmd("apex shell lock"), { description = "Lock session" })

-- ── Screenshots (grimblast) ──────────────────────────────────────────────────
bind("",  "Print", hl.dsp.exec_cmd(screenshot .. " area"),   { description = "Screenshot area" })
bind(mod, "Print", hl.dsp.exec_cmd(screenshot .. " screen"), { description = "Screenshot screen" })

-- ── Layout ───────────────────────────────────────────────────────────────────
bind(mod,           "F",     hl.dsp.window.fullscreen({ mode = 0 }),   { description = "Fullscreen" })
bind(mod .. " SHIFT", "SPACE", hl.dsp.window.float({ action = "toggle" }), { description = "Toggle floating" })
bind(mod,           "P",     hl.dsp.window.pseudo(),                   { description = "Pseudotile" })
bind(mod,           "J",     hl.dsp.layout("togglesplit"),             { description = "Toggle split" })

-- Focus with the arrow keys. Vim hjkl focus was removed: SUPER+l collided with
-- SUPER+L (lock), and letter keys are better left free for app/popup binds.
--
-- hyprlang took one-letter directions (`movefocus, l`); the Lua dispatcher takes
-- the direction spelled out.
local directions = { left = "left", right = "right", up = "up", down = "down" }
for key, direction in pairs(directions) do
    bind(mod, key, hl.dsp.focus({ direction = direction }),
         { description = "Focus " .. direction })
    bind(mod .. " SHIFT", key, hl.dsp.window.move({ direction = direction }),
         { description = "Move window " .. direction })
end

-- ── Workspaces ───────────────────────────────────────────────────────────────
-- 1-9 then 0, where 0 is workspace 10.
for i = 1, 10 do
    local key = tostring(i % 10)
    bind(mod, key, hl.dsp.focus({ workspace = i }),
         { description = "Workspace " .. i })
    bind(mod .. " SHIFT", key, hl.dsp.window.move({ workspace = i }),
         { description = "Move window to workspace " .. i })
end

-- Scratchpad / special workspace
bind(mod,             "S", hl.dsp.workspace.toggle_special("magic"),
     { description = "Toggle scratchpad" })
bind(mod .. " SHIFT", "S", hl.dsp.window.move({ workspace = "special:magic" }),
     { description = "Move window to scratchpad" })

-- Cycle workspaces with the mouse wheel
bind(mod, "mouse_down", hl.dsp.focus({ workspace = "e+1" }))
bind(mod, "mouse_up",   hl.dsp.focus({ workspace = "e-1" }))

-- ── Mouse drag move/resize ───────────────────────────────────────────────────
-- hyprlang's `bindm`. NOT `{ mouse = true }`: that option is accepted and does
-- nothing on 0.56.2 — the resulting keybind reports `mouse=false`, `drag=false`.
-- Upstream's own /usr/share/hypr/hyprland.lua still uses it. `{ drag = true }`
-- is what actually sets the press-and-hold behaviour (it implies `release`).
bind(mod, "mouse:272", hl.dsp.window.drag(),   { drag = true, description = "Move window" })
bind(mod, "mouse:273", hl.dsp.window.resize(), { drag = true, description = "Resize window" })

-- ── Media and brightness keys ────────────────────────────────────────────────
-- CTRL+SUPER equivalents for keyboards without these keys are shell defaults,
-- so they arrive through apex/shell-keybinds.lua.
--
-- hyprlang `bindel` = locked + repeating (fires with the screen locked or off,
-- and repeats while held). `bindl` = locked only. Volume and brightness are
-- useless without the repeat; the transport keys must stay one-shot.
local held   = { locked = true, repeating = true }
local locked = { locked = true }

bind("", "XF86AudioRaiseVolume",  hl.dsp.exec_cmd("wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%+"), held)
bind("", "XF86AudioLowerVolume",  hl.dsp.exec_cmd("wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%-"), held)
bind("", "XF86MonBrightnessUp",   hl.dsp.exec_cmd("brightnessctl set 5%+"), held)
bind("", "XF86MonBrightnessDown", hl.dsp.exec_cmd("brightnessctl set 5%-"), held)

bind("", "XF86AudioMute",    hl.dsp.exec_cmd("wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle"),   locked)
bind("", "XF86AudioMicMute", hl.dsp.exec_cmd("wpctl set-mute @DEFAULT_AUDIO_SOURCE@ toggle"), locked)

-- Transport keys. These were only in the labwc session, which the two configs
-- are meant to keep in step.
bind("", "XF86AudioPlay", hl.dsp.exec_cmd("playerctl play-pause"), locked)
bind("", "XF86AudioNext", hl.dsp.exec_cmd("playerctl next"),       locked)
bind("", "XF86AudioPrev", hl.dsp.exec_cmd("playerctl previous"),   locked)

return M
