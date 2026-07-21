import QtQuick
import Quickshell.Io

// ─────────────────────────────────────────────────────────────
// GreetContext — shared, non-visual greeter state.
//
// One instance lives at the shell root; every per-output GreetSurface
// binds to it. Holds the input buffers, the live clock, and (in later
// commits) the Greetd auth backend and session list. Keeping the
// state here — rather than per-surface as the Lockscreen did — means a
// multi-output kiosk mirrors one coherent auth attempt.
// ─────────────────────────────────────────────────────────────

Item {
    id: ctx

    // ── Auth / input state ────────────────────────────────────────
    property string username:  ""
    property string password:  ""
    property bool   checking:   false   // auth conversation in flight
    property bool   hasError:   false   // last attempt failed
    property string errorText:  ""

    // Emitted on every failed attempt so each surface can shake its
    // card and clear the field (the Lockscreen did this inline; here it
    // fans out to N surfaces).
    signal failed()

    // ── Edition ("gaming" | "daily" | "mono") ─────────────────────
    property string edition: "mono"

    // ── Live clock (local ticker, mirrors the Lockscreen) ─────────
    property string timeText: Qt.formatDateTime(new Date(), "hh:mm")
    property string dateText: Qt.formatDateTime(new Date(), "dddd, d MMMM")
    Timer {
        interval: 1000
        running:  true
        repeat:   true
        onTriggered: {
            var now = new Date()
            ctx.timeText = Qt.formatDateTime(now, "hh:mm")
            ctx.dateText = Qt.formatDateTime(now, "dddd, d MMMM")
        }
    }

    // ── Helpers ───────────────────────────────────────────────────
    function fail(msg) {
        ctx.password  = ""
        ctx.checking  = false
        ctx.hasError  = true
        ctx.errorText = msg
        ctx.failed()
    }

    // Submit. The real auth backend is wired in a subsequent commit;
    // this validates input and holds the guard structure.
    function tryAuth() {
        if (ctx.checking) return
        if (ctx.username.length === 0) { ctx.fail("Enter a username"); return }
        if (ctx.password.length === 0) return
        ctx.hasError  = false
        ctx.errorText = ""
        // (Greetd conversation added in a later commit)
    }

    // ── Edition detection ─────────────────────────────────────────
    // Precedence: /etc/apex-greet/edition override → /etc/os-release
    // VARIANT_ID → "mono" fallback (dev box / unset). Non-fatal.
    Process {
        id: editionProc
        running: true
        command: ["sh", "-c",
            "if [ -r /etc/apex-greet/edition ]; then head -n1 /etc/apex-greet/edition;" +
            " elif [ -r /etc/os-release ]; then . /etc/os-release; echo \"${VARIANT_ID:-}\"; fi"]
        stdout: SplitParser {
            onRead: function(line) {
                var v = line.trim().toLowerCase()
                if (v === "gaming" || v === "daily") ctx.edition = v
                else if (v !== "")                   ctx.edition = "mono"
            }
        }
    }
}
