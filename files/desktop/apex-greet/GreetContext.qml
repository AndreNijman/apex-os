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

    // ── Session model ─────────────────────────────────────────────
    // sessions: [{ id, name, exec }] parsed from wayland-sessions.
    property var    sessions:     []
    property int    sessionIndex: 0
    property string _wantSession: ""    // desired id from last-session

    readonly property var currentSession:
        (sessions.length > 0 && sessionIndex >= 0 && sessionIndex < sessions.length)
            ? sessions[sessionIndex] : null
    readonly property string sessionName: currentSession ? currentSession.name : "Default"
    // Command line of the selected session; a login shell is the safe
    // fallback when no wayland-sessions exist (e.g. the dev box).
    readonly property string sessionCommand: currentSession ? currentSession.exec : ""

    function cycleSession(dir) {
        var n = ctx.sessions.length
        if (n === 0) return
        ctx.sessionIndex = ((ctx.sessionIndex + dir) % n + n) % n
    }

    function _addSession(line) {
        var t = line.trim()
        if (t === "") return
        var parts = t.split("\t")
        if (parts.length < 3) return
        var exec = parts[2].replace(/%[a-zA-Z]/g, "").trim()   // strip field codes
        if (exec === "") return
        var list = ctx.sessions.slice()
        list.push({ id: parts[0], name: parts[1], exec: exec })
        ctx.sessions = list
        ctx._selectWanted()
    }

    function _selectWanted() {
        if (ctx._wantSession === "") return
        for (var i = 0; i < ctx.sessions.length; i++)
            if (ctx.sessions[i].id === ctx._wantSession) { ctx.sessionIndex = i; return }
    }

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
    //
    // last-user / last-session are written FIRST (values passed via env
    // so a hostile username can't inject shell), and the actual launch
    // is deferred to persistProc.onExited so the write flushes before
    // quickshell exits. An unwritable state dir is tolerated.
    property var _pendingArgv: null
    function launch() {
        var cmd  = ctx.sessionCommand.trim()
        var argv = cmd.length > 0 ? ["sh", "-lc", cmd]
                                  : ["sh", "-lc", "exec ${SHELL:-/bin/sh} -l"]
        ctx._pendingArgv = argv
        persistProc.environment = {
            "AG_USER": ctx.username,
            "AG_SESS": ctx.currentSession ? ctx.currentSession.id : ""
        }
        persistProc.command = ["sh", "-c",
            "d=/var/lib/apex-greet; mkdir -p \"$d\" 2>/dev/null;" +
            " printf '%s' \"$AG_USER\" > \"$d/last-user\" 2>/dev/null || true;" +
            " printf '%s' \"$AG_SESS\" > \"$d/last-session\" 2>/dev/null || true"]
        persistProc.running = true
    }

    Process {
        id: persistProc
        onExited: function(exitCode, exitStatus) {
            if (ctx._pendingArgv) {
                var a = ctx._pendingArgv
                ctx._pendingArgv = null
                Greetd.launch(a)
            }
        }
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

    // ── Last user (prefill) ───────────────────────────────────────
    Process {
        running: true
        command: ["sh", "-c", "cat /var/lib/apex-greet/last-user 2>/dev/null; echo"]
        stdout: SplitParser {
            onRead: function(line) {
                var u = line.trim()
                if (u !== "" && ctx.username === "") ctx.username = u
            }
        }
    }

    // ── Last session (preselect once the list is parsed) ──────────
    Process {
        running: true
        command: ["sh", "-c", "cat /var/lib/apex-greet/last-session 2>/dev/null; echo"]
        stdout: SplitParser {
            onRead: function(line) {
                var s = line.trim()
                if (s !== "") { ctx._wantSession = s; ctx._selectWanted() }
            }
        }
    }

    // ── Session list from /usr/share/wayland-sessions/*.desktop ───
    Process {
        running: true
        command: ["sh", "-c",
            "for f in /usr/share/wayland-sessions/*.desktop; do" +
            " [ -r \"$f\" ] || continue;" +
            " id=$(basename \"$f\" .desktop);" +
            " name=$(sed -n 's/^Name=//p' \"$f\" | head -n1);" +
            " exec=$(sed -n 's/^Exec=//p' \"$f\" | head -n1);" +
            " [ -n \"$exec\" ] || continue;" +
            " printf '%s\\t%s\\t%s\\n' \"$id\" \"${name:-$id}\" \"$exec\";" +
            " done"]
        stdout: SplitParser {
            onRead: function(line) { ctx._addSession(line) }
        }
    }
}
