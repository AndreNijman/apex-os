pragma Singleton
import QtQuick
import "."

QtObject {
    id: root

    // ── Color loader — watches matugen output and updates live ────────────────
    // Use a unique ID to avoid namespace collision with the 'Colors' singleton
    property var _loader: ColorLoader { id: internalLoader }

    // ── Colors — bound to loader, update automatically when matugen runs ──────
    property color background: internalLoader.background
    property color active:     internalLoader.active
    property color text:       internalLoader.text
    property color subtext:    internalLoader.subtext
    property color icon:       internalLoader.icon
    property color border:     internalLoader.border
    property color iconFont:   internalLoader.iconFont

    // ── Is the surface we are painting on dark? ───────────────────────────────
    // The one thing a status colour has to know about the palette. Relative
    // luminance per WCAG 2.1, threshold 0.18 — the luminance of a mid grey, so
    // the answer flips where a reader's eye does.
    //
    // Gamma has to be undone first: a colour's .r/.g/.b are sRGB components, and
    // averaging those directly calls #808080 "light" and #767676 "dark" when
    // both are mid grey. That error is invisible on the extremes and wrong
    // exactly where the threshold lives.
    function channelLuminance(v) {
        return v <= 0.04045 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4)
    }
    function luminance(c) {
        return 0.2126 * channelLuminance(c.r)
             + 0.7152 * channelLuminance(c.g)
             + 0.0722 * channelLuminance(c.b)
    }
    readonly property bool darkSurface: luminance(root.background) < 0.18

    // ── Status ────────────────────────────────────────────────────────────────
    // Fixed hues, chosen per surface lightness. NOT loaded from the palette,
    // and that is the whole point.
    //
    // matugen gives us `error`, so `danger` COULD follow the wallpaper. The
    // others have no Material You role at all — there is no guaranteed-amber or
    // guaranteed-green in an M3 scheme — so sourcing one and fixing the rest
    // would make the status family internally incoherent: a red that moves with
    // the wallpaper next to a green that does not.
    //
    // A status colour also has a job that outranks harmony. "Disk 94% full" and
    // "battery at 4%" have to read as alarming on every wallpaper, including a
    // red one, where a harmonised error colour is at its least legible.
    //
    // WHY THERE ARE TWO OF EACH. Hue is fixed; lightness is not. A #a6e3a1 green
    // is a 11.8:1 read on a dark surface and a 1.3:1 read on a light one, which
    // is not a green a person can see — it is a pale smear. matugen emits a
    // light scheme on request (`-m light`, `surface` #faf9fb), so the pair is
    // the difference between a status family that works on both and one that
    // only ever got looked at on the maintainer's own wallpaper.
    //
    // ── LIGHT MODE IS NOT SUPPORTED YET, AND THIS IS WHERE THAT IS RECORDED ──
    //
    // The light values below are measured, not guessed: tests/agent-state-test.js
    // checks all twelve palettes — six shipped wallpapers, both matugen modes —
    // and tests/run-agent-state-render-test.sh drives both through the real
    // Theme in a headless compositor, worst light contrast 4.81:1. They are
    // correct. They are also not something a user should be able to switch on
    // today, and the reason is not in this file:
    //
    //   212 `color:` bindings across src/ are Qt.rgba(1, 1, 1, α)
    //
    // — a translucent white foreground, which reads on the dark surface that has
    // always been the only reachable one and is invisible on matugen's light
    // surface (#fdf9f3 on the default wallpaper). 20 of them are in the settings
    // pages, including the description text under every section heading. A
    // Light/Dark toggle would hand the user a blank Settings window. So APEX
    // Shell is a dark shell, on purpose, until those are tokens.
    //
    // It is REACHABLE, so the palette above is not dead code and can be worked
    // on: WallpaperService passes matugen `-m <mode>` and takes the mode from
    // ~/.config/apex-shell/src/user_data/wallpaper.json. Set `"mode": "light"`
    // there and re-apply a wallpaper. There is no control in Settings, by the
    // paragraph above.
    //
    // tests/check-color-tokens.sh counts those 212 sites and fails if the number
    // goes UP, so this comment cannot quietly stop being true and the debt can
    // only be paid down. When it reaches zero, wire the toggle back into
    // AppearancePage — WallpaperService.setMode() is already there.
    //
    // The dark values are byte-identical to the ones already dominant in the
    // tree, so nothing moves on an existing install: `danger` was #f87171 at 12
    // of the 24 red call sites, `warning` #f5c47a at 7 of 13, `success`
    // #a6e3a1 at 2 of 3.
    //
    // Contrast against the surfaces they are actually drawn on — the page
    // background, a card (background + 3% text) and that card hovered
    // (background + 7% text) — measured for both matugen schemes of the shipped
    // default wallpaper, in tests/agent-state-test.js. Every one clears 4.5:1.
    property color danger:    darkSurface ? "#f87171" : "#8c1d18"
    property color warning:   darkSurface ? "#f5c47a" : "#7a4a00"
    property color success:   darkSurface ? "#a6e3a1" : "#17752f"

    // Work in progress, and a hold only a person can release. Both are new with
    // the Agent Center's state colours (roadmap P0-021) and neither duplicates
    // an existing token: `active` is the wallpaper's primary and lands wherever
    // matugen puts it — on the light scheme of the default wallpaper it is
    // #000613, near-black — so it cannot carry a meaning that has to stay
    // recognisable across palettes.
    //
    // Blue and violet rather than two blues: after protanopia and deuteranopia
    // the pair still separates on the blue axis, and their luminances differ by
    // enough to survive greyscale.
    property color info:      darkSurface ? "#89b4fa" : "#0b57d0"
    property color attention: darkSurface ? "#d0bcff" : "#6b3fa0"

    // The foreground for a glyph or label sitting ON one of the fills above.
    //
    // Whichever of the two fixed foregrounds actually measures better, rather
    // than a lightness threshold. A threshold has to be placed, and every place
    // to put one is wrong somewhere: at 0.35 it sends white onto #f87171, which
    // is a 2.8:1 read, while the dark foreground next to it would have been
    // 5.9:1. Comparing the two candidates has no such seam and cannot drift
    // when a token's value changes.
    function contrastRatio(a, b) {
        var ya = luminance(a)
        var yb = luminance(b)
        return ya > yb ? (ya + 0.05) / (yb + 0.05)
                       : (yb + 0.05) / (ya + 0.05)
    }
    function onStatus(fill) {
        return contrastRatio(root.fixedDark, fill)
               >= contrastRatio(root.fixedLight, fill)
               ? root.fixedDark : root.fixedLight
    }

    // ── Fixed-contrast foregrounds ────────────────────────────────────────────
    // For a foreground sitting on something whose colour is NOT the themed
    // background: a knob on a filled slider track, a label on a status chip, a
    // glyph on a fixed dark overlay. Theme.text would follow the palette and
    // could land at the same lightness as the fill underneath it, so these two
    // stay put on purpose. Named for the constraint, not for a surface, so
    // nobody reads them as "the accent's foreground" and reuses them wrongly.
    property color fixedLight: "#ffffff"
    property color fixedDark:  "#1e1e2e"

    // ── Destructive filled control ────────────────────────────────────────────
    // The confirm button on a "this deletes things" dialog. A *fill*, so it is
    // much darker than `danger`, which is a foreground accent — they are not
    // interchangeable. Already identical in ConfirmDialog and KanbanBoard before
    // this became a token; a token is what stops them drifting apart.
    property color dangerFill:      "#993030"
    property color dangerFillHover: "#cc3a3a"

    // --- Workspace Visuals ---
    property color wsBackground: "#20000000"
    property color wsActive:     "#FFFFFF"
    property color wsOccupied:   "#80FFFFFF"
    property color wsEmpty:      "#30FFFFFF"
    property color wsOverlay:    "#CC1e1e2e"
    property color wsUrgent:     "#fa6b94"
}
