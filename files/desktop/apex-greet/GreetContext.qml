import QtQuick
import Quickshell.Io
import Quickshell.Services.Greetd

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

    // Command line of the selected session. Empty until the session
    // picker (added in a later commit) populates it; a login shell is
    // used as a safe fallback so the auth flow is testable on its own.
    property string sessionCommand: ""

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

    // ── Greetd auth conversation ──────────────────────────────────
    //
    // PamContext → Greetd mapping (the Lockscreen used PAM directly; a
    // greeter must talk to greetd, which owns the PAM conversation):
    //
    //   Lockscreen (PAM)                 apex-greet (Greetd)
    //   ────────────────                 ───────────────────
    //   pam.start()                      Greetd.createSession(username)
    //   onResponseRequired → respond()   onAuthMessage(responseRequired)
    //                                        → Greetd.respond(password)
    //   onCompleted(Success) → unlock    onReadyToLaunch → Greetd.launch(argv)
    //   onCompleted(fail)    → fail()    onAuthFailure   → fail()
    //   onError              → fail()    onError         → fail()
    //
    // Submit: begins (or continues) the conversation.
    function tryAuth() {
        if (ctx.checking) return
        if (ctx.username.length === 0) { ctx.fail("Enter a username"); return }
        if (ctx.password.length === 0) return
        if (!Greetd.available)         { ctx.fail("greetd is not available"); return }
        ctx.hasError  = false
        ctx.errorText = ""
        ctx.checking  = true
        if (Greetd.state === GreetdState.Inactive) {
            // Fresh attempt — greetd replies with an auth message that
            // onAuthMessage answers with the buffered password.
            Greetd.createSession(ctx.username)
        } else {
            // A prompt is already outstanding (rare) — answer it directly.
            Greetd.respond(ctx.password)
        }
    }

    // Hand the chosen session to greetd. greetd opens the PAM session
    // and execs the command; quickshell exits. A login shell wraps the
    // Exec line so PATH / profile resolve, mirroring common greeters.
    function launch() {
        var cmd  = ctx.sessionCommand.trim()
        var argv = cmd.length > 0 ? ["sh", "-lc", cmd]
                                  : ["sh", "-lc", "exec ${SHELL:-/bin/sh} -l"]
        Greetd.launch(argv)
    }

    Connections {
        target: Greetd

        // greetd relays a PAM prompt. Hidden prompts (echoResponse
        // false) are the password; visible prompts get the username.
        // error-type messages are surfaced without ending the session.
        function onAuthMessage(message, error, responseRequired, echoResponse) {
            if (responseRequired) {
                Greetd.respond(echoResponse ? ctx.username : ctx.password)
            } else if (error) {
                ctx.hasError  = true
                ctx.errorText = message
            }
        }

        // The one and only success path.
        function onReadyToLaunch() {
            ctx.launch()
        }

        // greetd has already torn the session down; reset to a clean
        // Inactive state so the next Enter starts fresh.
        function onAuthFailure(message) {
            ctx.fail(message && message.length > 0 ? message : "Wrong password")
        }

        function onError(message) {
            if (Greetd.state !== GreetdState.Inactive) Greetd.cancelSession()
            ctx.fail("Auth unavailable: " + message)
        }
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
