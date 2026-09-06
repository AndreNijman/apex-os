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
-- ── These values are not free ────────────────────────────────────────────────
-- Every one of them must equal the matching entry in apex-input-apply's
-- DEFAULTS, and tests/test-apex-input.sh asserts that key by key.
--
-- The reason is the first use of Settings → Input. That page writes the WHOLE
-- model, so touching one control sends all of them to the compositor. Where the
-- two disagree, changing tap-to-click also turns on whatever else the model
-- happens to default to — and this file used to set two touchpad options out of
-- ten, so a user who moved one slider silently gained drag lock,
-- disable-while-typing and clickfinger clicking.
--
-- Anything absent here is absent because Hyprland's touchpad block has no
-- option for it: pointer speed, acceleration, left-handed and scroll method for
-- a touchpad exist only as hl.device fields, and there is no device to name
-- until the machine has one.
hl.config({
    input = {
        kb_layout  = "@KB_LAYOUT@",
        kb_variant = "@KB_VARIANT@",

        follow_mouse = 1,

        -- Mouse and keyboard. `sensitivity` and `accel_profile` here are the
        -- POINTER's: the touchpad's own are per-device and cannot be seeded.
        sensitivity    = 0.0,
        accel_profile  = "adaptive",
        natural_scroll = false,
        left_handed    = false,
        scroll_factor  = 1.0,
        repeat_rate    = 25,
        repeat_delay   = 600,

        touchpad = {
            -- hyprlang spelled the first two `tap-to-click` and
            -- `tap-and-drag`; the Lua keys are underscored, and an unknown key
            -- is rejected outright rather than ignored, which is what makes the
            -- build-time verify useful.
            natural_scroll          = true,
            tap_to_click            = true,
            tap_and_drag            = true,
            drag_lock               = true,
            disable_while_typing    = true,
            middle_button_emulation = false,
            clickfinger_behavior    = true,
            tap_button_map          = "lrm",
            scroll_factor           = 1.0,
            -- 0 off, 1 three fingers, 2 four.
            drag_3fg                = 0,
        },
    },
})

-- Touchpad workspace swipe. hyprlang: `gesture = 3, horizontal, workspace`.
hl.gesture({ fingers = 3, direction = "horizontal", action = "workspace" })
