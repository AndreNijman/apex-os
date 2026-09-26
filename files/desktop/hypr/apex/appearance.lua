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

-- ── Motion ───────────────────────────────────────────────────────────────────
-- The compositor moves on the same beats and curves as APEX Shell (UI/UX
-- roadmap v3 Phase 20, retuned 2026-09-26 with the shell's fluid motion), so a
-- window opening and a panel opening read as one system. The numbers are the
-- shell's own tokens (apex-shell src/theme/motion.js, at the default
-- "balanced" speed); a speed here is in tenths of a second. The shell scales
-- every one of them by its own speed setting at runtime (hyprMotion.js).
--
-- Andre: "everything goes way too quick and doesn't feel liquid and fluid."
-- The first tuning (240 ms opens on a curve with a near-vertical first frame)
-- snapped windows into place. Nothing physical starts at full speed:
--
--   arriving  a critically damped spring from rest, fitted   spring
--   leaving   eases in and out, shorter                      standardAccel
--   moving    the same spring: a tile glides to its place    spring
--   fading    Core Animation's ease, no hard edge            effects
--
-- hyprlang carried these as `bezier =` / `animation =` keys; each is its own
-- call now, `enabled / speed / curve` becoming named fields.
hl.curve("apexSpring",   { type = "bezier", points = { { 0.25, 0.2 }, { 0.15, 1.0 } } })
hl.curve("apexAccel",    { type = "bezier", points = { { 0.4, 0.0 }, { 0.65, 1.0 } } })
hl.curve("apexStandard", { type = "bezier", points = { { 0.25, 0.1 }, { 0.25, 1.0 } } })
hl.curve("apexEffects",  { type = "bezier", points = { { 0.25, 0.1 }, { 0.25, 1.0 } } })

-- Windows: in over 420 ms, out over 280, both from 90 % so a window grows into
-- place rather than zooming from a dot; a window moving (a tile reflowing, a
-- float dragged into a slot) glides on the page beat, 380 ms.
hl.animation({ leaf = "windowsIn",   enabled = true, speed = 4.2, bezier = "apexSpring", style = "popin 90%" })
hl.animation({ leaf = "windowsOut",  enabled = true, speed = 2.8, bezier = "apexAccel",  style = "popin 90%" })
hl.animation({ leaf = "windowsMove", enabled = true, speed = 3.8, bezier = "apexSpring" })

-- Fades: a window's opacity on the shell's fadeIn (280 ms) and, closing, over
-- the length of its exit so the popin is seen; focus and dim changes on the
-- state beat (220 ms); an application's own menus quicker (micro, 140 ms).
hl.animation({ leaf = "fadeIn",     enabled = true, speed = 2.8, bezier = "apexEffects" })
hl.animation({ leaf = "fadeOut",    enabled = true, speed = 2.8, bezier = "apexAccel" })
hl.animation({ leaf = "fadeSwitch", enabled = true, speed = 2.2, bezier = "apexEffects" })
hl.animation({ leaf = "fadeShadow", enabled = true, speed = 2.2, bezier = "apexEffects" })
hl.animation({ leaf = "fadeDim",    enabled = true, speed = 2.2, bezier = "apexEffects" })
hl.animation({ leaf = "fadePopups", enabled = true, speed = 1.4, bezier = "apexEffects" })

-- Other programs' layer surfaces (a wallpaper daemon, an input method, a
-- third-party launcher) fade on the small-surface beats. APEX Shell's own are
-- exempt below: it draws every one of their motions itself.
hl.animation({ leaf = "layersIn",      enabled = true, speed = 3.6, bezier = "apexSpring",  style = "fade" })
hl.animation({ leaf = "layersOut",     enabled = true, speed = 2.4, bezier = "apexAccel",   style = "fade" })
hl.animation({ leaf = "fadeLayersIn",  enabled = true, speed = 3.6, bezier = "apexEffects" })
hl.animation({ leaf = "fadeLayersOut", enabled = true, speed = 2.4, bezier = "apexAccel" })

-- Workspaces: a keyboard switch slides on the page beat (380 ms) and lands on
-- the spring's long tail. A touchpad swipe (input-defaults.lua) follows the
-- fingers directly and uses this curve only for the settle after release.
hl.animation({ leaf = "workspaces",       enabled = true, speed = 3.8, bezier = "apexSpring", style = "slide" })
hl.animation({ leaf = "specialWorkspace", enabled = true, speed = 3.6, bezier = "apexSpring", style = "slidefadevert 15%" })

-- A focus change re-colours the border on the state beat; it was 600 ms.
hl.animation({ leaf = "border", enabled = true, speed = 2.2, bezier = "apexStandard" })

-- ── APEX Shell's surfaces are not animated by the compositor ────────────────
-- Every one of them — the bar, a panel pouring out of it, the OSD, a toast —
-- is drawn in motion by the shell, frame by frame, and a panel's first frame
-- is pixel-identical to the bar it grows out of. Hyprland fades every layer
-- surface in on map (fadeLayersIn, inherited from `fade`), so without this a
-- panel poured out of the bar translucent: measured in a nested Hyprland
-- 0.56.2, 66-90 % of the network panel's footprint was a blend of panel and
-- wallpaper from 240 to 345 ms into its open, and it snapped opaque at 400 ms.
-- With the rule, the blend is the shell's own content fade at its edges (<= 7 %).
-- The shell's layers all carry Quickshell's default namespace.
hl.layer_rule({
    name  = "apex-shell-draws-its-own-motion",
    match = { namespace = "^quickshell$" },

    no_anim = true,
})
