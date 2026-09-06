-- APEX look and feel. Loaded by ~/.config/hypr/hyprland.lua.
--
-- APEX-owned: `apex update` replaces this file. Put your own overrides in
-- hyprland.lua, which is yours and is never rewritten — it loads after nothing
-- and before nothing that would undo them, because hl.config is last-write-wins.

hl.config({
    general = {
        gaps_in     = 5,
        gaps_out    = 10,
        border_size = 2,

        -- The active border is re-tinted from the wallpaper by APEX Shell's
        -- matugen pipeline (WallpaperService.updateBorders), which pushes the
        -- new colour over `hyprctl keyword` at runtime. These are the values a
        -- session starts with before any wallpaper has been picked.
        --
        -- The hyprlang spelling was one string with a trailing `45deg`:
        --     col.active_border = rgba(a1c999ee) rgba(8d748cee) 45deg
        -- A gradient in Lua is a table of stops plus an angle instead.
        col = {
            active_border   = { colors = { "rgba(a1c999ee)", "rgba(8d748cee)" }, angle = 45 },
            inactive_border = "rgba(252733aa)",
        },

        layout           = "dwindle",
        resize_on_border = true,
    },

    decoration = {
        rounding = 10,
        blur   = { enabled = true, size = 6, passes = 2 },
        shadow = { enabled = true, range = 8, render_power = 3 },
    },

    animations = {
        enabled = true,
    },

    dwindle = {
        preserve_split = true,
    },

    misc = {
        disable_hyprland_logo    = true,
        disable_splash_rendering = true,
        force_default_wallpaper  = 0,
    },
})

-- Curves and animations are their own calls now; hyprlang carried them as
-- `bezier =` and `animation =` keys inside the animations block.
--
--     bezier    = ease, 0.25, 0.1, 0.25, 1.0
--     animation = windows, 1, 4, ease
--
-- becomes a named curve plus one call per leaf, where the hyprlang positional
-- `1, 4, ease` is enabled / speed / curve.
hl.curve("ease", { type = "bezier", points = { { 0.25, 0.1 }, { 0.25, 1.0 } } })

hl.animation({ leaf = "windows",    enabled = true, speed = 4, bezier = "ease" })
hl.animation({ leaf = "fade",       enabled = true, speed = 4, bezier = "ease" })
hl.animation({ leaf = "workspaces", enabled = true, speed = 4, bezier = "ease" })
hl.animation({ leaf = "border",     enabled = true, speed = 6, bezier = "ease" })
