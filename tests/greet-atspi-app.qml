import QtQuick

// ─────────────────────────────────────────────────────────────────────────────
//  greet-atspi-app.qml — the shipped login surface, in a real window, so that
//  what it publishes to AT-SPI can be read off the bus.
//
//  Run via tests/test-apex-greet-atspi.sh. This is the companion to
//  greet-a11y-test.qml, and the difference between them is the whole point:
//
//    greet-a11y-test.qml  reads `Accessible.name` off a live QQuickItem under
//                         qmltestrunner. That proves the QML is right.
//    this file            puts the same surface in a window on a private
//                         compositor, with a private accessibility bus, and the
//                         suite then reads the tree back over D-Bus. That
//                         proves the BRIDGE publishes what the QML declared --
//                         which is a different claim, and the one the roadmap
//                         criterion ("screen reader ... validated") actually
//                         makes.
//
//  The two trees are not the same tree. Qt's bridge suppresses the name of any
//  passwordEdit item, maps roles into AT-SPI's vocabulary, and drops items it
//  considers uninteresting. None of that is visible from the QML side, and all
//  of it is what a screen reader user meets.
//
//  ── The counters ────────────────────────────────────────────────────────────
//
//  A screen reader does not only read; it presses. `org.a11y.atspi.Action.
//  DoAction` is how a reader activates a control, and a suite that called it
//  and checked only the boolean reply would be asserting that the bridge
//  ACCEPTED the call, not that anything happened -- DoAction returns true on
//  this surface even for a control whose handler does nothing.
//
//  So every side effect is published back onto the bus as an accessible name on
//  a probe node. The suite presses "Next session" over D-Bus and then re-reads
//  the tree; if the name has not moved, the press did not reach the handler.
//  The whole round trip stays inside the accessibility layer, which is the
//  layer the criterion is about.
// ─────────────────────────────────────────────────────────────────────────────

Window {
    id: win
    visible: true
    width: 1280
    height: 800
    title: "apex-greet-atspi-fixture"
    color: "#000000"

    // ── The fakes, kept to exactly what GreetSurface.qml reads ───────────────
    // Same shape as greet-a11y-test.qml's stubs. The real GreetContext needs
    // Quickshell.Services.Greetd, a greetd socket and a Process; a test may
    // have none of those, and authenticating anything here would be wrong even
    // if it could.
    QtObject {
        id: stubTheme
        readonly property string fontFamily: "DejaVu Sans"
        readonly property color background: "#1a282a"
        readonly property color text:       "#cdd6f4"
        readonly property color subtext:    "#94e2d5"
        readonly property color border:     "#ffffff"
        readonly property color errorColor: "#ff5c5c"
        readonly property color active:     "#a6d0f7"
        readonly property url   logoSource: ""
    }

    QtObject {
        id: stubCtx
        signal failed()

        property string username:  ""
        property string password:  ""
        property bool   checking:  false
        property bool   hasError:  false
        property string errorText: ""

        property var sessions: [ { id: "apex-labwc",  name: "APEX Desktop", exec: "x" },
                                 { id: "apex-gaming", name: "APEX Gaming",  exec: "y" } ]
        property int  sessionIndex: 0
        readonly property string sessionName: sessions[sessionIndex].name

        property string timeText: "09:41"
        property string dateText: "Friday, 12 September"
        readonly property url wallpaper: ""

        property string layoutName: "us"
        property var    layouts:    [ "us", "de" ]

        property int cycleCount:   0
        property int authCount:    0
        property int layoutCycles: 0

        // cycleSession really moves the index, so the effect of a press is
        // visible in `sessionName` -- which the surface publishes as an
        // accessible name. A stub that only counted would let the suite assert
        // a counter nobody but the suite can see.
        function cycleSession(dir) {
            cycleCount++
            var n = sessions.length
            sessionIndex = ((sessionIndex + dir) % n + n) % n
        }
        function tryAuth() { authCount++ }
        function cycleLayout(dir) {
            layoutCycles++
            var n = layouts.length
            var i = layouts.indexOf(layoutName)
            layoutName = layouts[((i + dir) % n + n) % n]
        }
    }

    // ── The surface under test: the shipped file, staged beside this one ─────
    GreetSurface {
        id: surface
        anchors.fill: parent
        theme: stubTheme
        ctx:   stubCtx
    }

    // ── A secret typed into the password field ──────────────────────────────
    // The suite asks the bus what org.a11y.atspi.Text.GetText returns for this
    // field. With the field EMPTY that question has a trivially safe answer and
    // the assertion proves nothing, so a known string is put in it and the
    // suite requires the bus to return the mask instead -- and, specifically,
    // never these characters. Found by objectName, and the search is required
    // to succeed: a fixture that silently failed to fill the field would make
    // the security assertion vacuous again, which is the exact failure this
    // paragraph exists to prevent.
    property string secret: "correct-horse-battery-staple"
    property bool   secretPlaced: false

    function findByName(node, want) {
        if (!node)
            return null
        if (node.objectName === want)
            return node
        var kids = node.children
        if (kids === undefined)
            return null
        for (var i = 0; i < kids.length; i++) {
            var hit = findByName(kids[i], want)
            if (hit)
                return hit
        }
        return null
    }

    Component.onCompleted: {
        var f = findByName(surface, "greetPasswordField")
        if (f) {
            f.text = win.secret
            win.secretPlaced = (f.text === win.secret)
        }
    }

    // Published so the suite can refuse to trust the masking assertion unless
    // the secret really went in.
    Text {
        objectName: "probeSecretPlaced"
        text: ""
        Accessible.role: Accessible.StaticText
        Accessible.name: "probe-secretPlaced=" + (win.secretPlaced ? "1" : "0")
    }

    // ── Probes, published onto the bus ──────────────────────────────────────
    // Not part of the greeter. These exist so that a press delivered over
    // AT-SPI has an effect the suite can READ over AT-SPI, closing the loop
    // without leaving the accessibility layer.
    Item {
        objectName: "atspiProbes"
        Text {
            objectName: "probeCycleCount"
            text: ""
            Accessible.role: Accessible.StaticText
            Accessible.name: "probe-cycleCount=" + stubCtx.cycleCount
        }
        Text {
            objectName: "probeAuthCount"
            text: ""
            Accessible.role: Accessible.StaticText
            Accessible.name: "probe-authCount=" + stubCtx.authCount
        }
        Text {
            objectName: "probeLayoutCycles"
            text: ""
            Accessible.role: Accessible.StaticText
            Accessible.name: "probe-layoutCycles=" + stubCtx.layoutCycles
        }
    }
}
