-- APEX window rules. Loaded by ~/.config/hypr/hyprland.lua.
--
-- APEX-owned: `apex update` replaces this file.

-- Ignore maximize requests from every app.
--
-- hyprlang: `windowrule = suppress_event maximize, match:class .*`
-- The rule now carries a `name`, which is what makes it addressable: the handle
-- returned here has `:set_enabled(false)`, so it can be turned off without
-- being deleted. hyprlang had no way to disable a rule once declared.
local suppress_maximize = hl.window_rule({
    name  = "apex-suppress-maximize",
    match = { class = ".*" },

    suppress_event = "maximize",
})

-- Fix XWayland drag-and-drop: an unmapped, untitled, floating XWayland surface
-- is the drag proxy, and focusing it breaks the drag.
--
-- hyprlang: `windowrule = match:class ^$, match:title ^$, match:xwayland 1,
--            match:float 1, match:fullscreen 0, match:pin 0, no_focus on`
local xwayland_drag = hl.window_rule({
    name  = "apex-fix-xwayland-drags",
    match = {
        class      = "^$",
        title      = "^$",
        xwayland   = true,
        float      = true,
        fullscreen = false,
        pin        = false,
    },

    no_focus = true,
})

-- Returned so hyprland.lua, or anything the user writes in it, can reach the
-- handles:
--
--     require("apex.rules").suppress_maximize:set_enabled(false)
return {
    suppress_maximize = suppress_maximize,
    xwayland_drag     = xwayland_drag,
}
