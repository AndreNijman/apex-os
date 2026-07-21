import QtQuick
import Quickshell
import Quickshell.Wayland

// ─────────────────────────────────────────────────────────────
// apex-greet — standalone greetd greeter for APEX-OS.
//
// A visual + interaction port of Brain_Shell's Lockscreen, made
// fully self-contained: none of the shell singletons the Lockscreen
// leaned on (Theme, WallpaperService, LockState) exist at greeter
// stage, so the palette, wallpaper, clock and all state are inlined
// here instead of imported.
//
// Runtime model: this runs under `cage` (a kiosk Wayland compositor)
// launched by greetd — NOT a session lock. cage implements
// wlr-layer-shell (the same path gtkgreet uses), so the fullscreen
// surface is a PanelWindow anchored to all four edges with Exclusive
// keyboard focus, one per output via Variants over Quickshell.screens.
//
// Auth backend is Quickshell.Services.Greetd (see GreetContext.qml),
// which replaces the Lockscreen's PamContext.
// ─────────────────────────────────────────────────────────────

ShellRoot {
    id: shell

    // ── Inlined palette (ThemeGreet) ──────────────────────────────
    // Values lifted verbatim from Brain_Shell's colours.json fallbacks
    // (src/theme/ColorLoader.qml) — the same defaults the Lockscreen
    // paints with before any live theme is loaded. `active` and the
    // spark logo are edition-derived so gaming / daily / dev each get
    // their own accent.
    QtObject {
        id: theme
        readonly property string fontFamily: "JetBrainsMono Nerd Font"
        readonly property color background: "#1a282a"
        readonly property color text:       "#cdd6f4"
        readonly property color subtext:    "#94e2d5"
        readonly property color border:     "#ffffff"
        readonly property color errorColor: "#ff5c5c"

        // Edition accent: gaming → gold, daily → chartreuse, anything
        // else (dev box / VARIANT_ID unset) → the Brain_Shell blue so
        // the greeter is pixel-identical to the Lockscreen off-target.
        readonly property color active:
            ctx.edition === "gaming" ? "#fde047"
          : ctx.edition === "daily"  ? "#d9f99d"
          : "#a6d0f7"

        // Assets ship alongside this file, so Qt.resolvedUrl works both
        // in-tree (dev) and installed at /usr/share/apex-greet/.
        readonly property url logoSource:
            ctx.edition === "gaming" ? Qt.resolvedUrl("assets/spark-gold.png")
          : ctx.edition === "daily"  ? Qt.resolvedUrl("assets/spark-chartreuse.png")
          : Qt.resolvedUrl("assets/spark-white.png")
    }

    // ── Shared greeter state + auth backend ───────────────────────
    GreetContext { id: ctx }

    // ── One fullscreen layer-shell surface per output ─────────────
    Variants {
        model: Quickshell.screens
        PanelWindow {
            id: win
            required property var modelData
            screen: modelData

            // Opaque base so there is never a transparent flash before
            // the wallpaper / gradient paints.
            color: "black"

            WlrLayershell.layer:        WlrLayer.Overlay
            WlrLayershell.keyboardFocus: WlrKeyboardFocus.Exclusive
            WlrLayershell.namespace:     "apex-greet"

            // All four edges anchored → fills the output.
            anchors {
                top:    true
                bottom: true
                left:   true
                right:  true
            }

            GreetSurface {
                anchors.fill: parent
                theme: theme
                ctx:   ctx
            }
        }
    }
}
