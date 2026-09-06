-- APEX input defaults. Loaded by ~/.config/hypr/hyprland.lua BEFORE the
-- generated apex/input.lua, so anything set in APEX Settings → Input wins and
-- these are the fallback for a machine where that page has never been used.
--
-- APEX-owned: `apex update` replaces this file.

-- ── Keyboard layout ──────────────────────────────────────────────────────────
-- The layout and variant below are substituted by apex-shell-firstrun from the
-- SYSTEM keymap (/etc/X11/xorg.conf.d/00-keyboard.conf, i.e. whatever
-- `localectl set-x11-keymap` or the installer recorded), falling back to `us`
-- when the system has no keymap set. Hyprland does not read that file itself.
-- Hardcoding `us` meant a German, French or Spanish user got a US layout no
-- matter what they chose at install time — and had to type their password on it.
--
-- This comment deliberately does NOT name the two placeholder tokens, and does
-- not quote the value localectl prints for an unset keymap either. Both were
-- learned the hard way:
--   1. the seeding `sed` is a plain global substitution, so writing the tokens
--      here got them replaced in the COMMENT too, and every seeded config
--      carried the bug's own value pasted into its own documentation;
--   2. apex-shell-firstrun's postcondition greps seeded configs for that
--      placeholder value, so quoting it verbatim made a correct, brand-new
--      install fail its own provisioning check (apex-logs 14).
-- The check strips comments before matching now, but keeping both strings out
-- of the file means neither mistake can come back.
hl.config({
    input = {
        kb_layout  = "@KB_LAYOUT@",
        kb_variant = "@KB_VARIANT@",

        follow_mouse = 1,
        sensitivity  = 0,

        touchpad = {
            -- hyprlang spelled this `tap-to-click`; the Lua key is
            -- `tap_to_click`, and an unknown key is rejected outright rather
            -- than ignored, which is what makes the build-time verify useful.
            natural_scroll = true,
            tap_to_click   = true,
        },
    },
})

-- Touchpad workspace swipe. hyprlang: `gesture = 3, horizontal, workspace`.
hl.gesture({ fingers = 3, direction = "horizontal", action = "workspace" })
