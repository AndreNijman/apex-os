import QtQuick
import QtTest

// ─────────────────────────────────────────────────────────────────────────────
//  greet-a11y-test.qml — what a screen reader and a keyboard-only user actually
//  get from the login screen (roadmap P2-003 "accessible login", and the
//  greeter half of P2-004's "keyboard layout before password").
//
//  Run via tests/test-apex-greet-a11y.sh, which stages the SHIPPED
//  files/desktop/apex-greet/GreetSurface.qml next to theme and ctx stubs and
//  hands this file to qmltestrunner on the offscreen platform. The surface
//  under test is the shipped file byte for byte; only the palette it paints
//  with and the auth backend it talks to are fakes, and no assertion here looks
//  at a colour or authenticates anything.
//
//  ── Why this is not a grep ──────────────────────────────────────────────────
//
//  "Does the greeter have accessible names" can be answered by grepping for
//  `Accessible.name`, and that answer is worth nothing: the property can be
//  present and bound to an empty string, present on a wrapper the screen reader
//  never reaches, or present on four of six controls. So every assertion in
//  here READS THE LIVE OBJECT — `item.Accessible.name` resolved on a walked
//  instance, which is the same attached object QAccessible publishes to AT-SPI
//  — and every focus assertion POSTS A REAL Qt.Key_Tab through the window's
//  delivery agent rather than reading `KeyNavigation.tab` out of the source.
//
//  A worked example of the difference: `KeyNavigation.tab: passwordInput` on
//  the username field looks like a complete Tab chain in the source. Pressing
//  Tab shows it is a two-element ring — the session picker, which is a pair of
//  Text items with MouseArea children, is not in it at all, so a keyboard-only
//  user cannot reach the control that chooses which desktop they log into.
//
//  ── Finding controls ────────────────────────────────────────────────────────
//
//  By objectName, never by label text or by shape. A lookup that finds the
//  password field by `echoMode === TextInput.Password` stops finding it the day
//  someone changes the echo mode, and an assertion whose subject has vanished
//  passes instead of failing. Every lookup here is therefore checked for a hit
//  first — `verify(it !== null)` — and the Tab-chain assertions compare against
//  an expected ORDERED LIST whose length is itself asserted, so deleting a
//  control shortens the chain and fails rather than quietly narrowing the test.
// ─────────────────────────────────────────────────────────────────────────────

Item {
    id: fixture
    width:  1280
    height: 800

    // ── The fakes ────────────────────────────────────────────────────────────
    // Only what GreetSurface.qml reads. Kept minimal on purpose: a stub that
    // grows members the surface does not use is a stub nobody can check against
    // the real GreetContext.
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

        property var    sessions:    [ { id: "apex-labwc", name: "APEX Desktop", exec: "x" },
                                       { id: "apex-gaming", name: "APEX Gaming", exec: "y" } ]
        property int    sessionIndex: 0
        readonly property string sessionName: sessions[sessionIndex].name

        property string timeText: "09:41"
        property string dateText: "Friday, 12 September"
        readonly property url wallpaper: ""

        // The keyboard layout the session came up on. Read by the surface's
        // layout indicator; the fixture drives it to prove the indicator is
        // bound to the context rather than to a literal.
        property string layoutName:   "us"
        property var    layouts:      [ "us" ]

        property int cycleCount:  0
        property int authCount:   0
        property int layoutCycles: 0

        function cycleSession(dir) { cycleCount++ }
        function tryAuth()         { authCount++ }
        function cycleLayout(dir)  { layoutCycles++ }
    }

    // ── Premise probes ───────────────────────────────────────────────────────
    // Not part of the greeter. These pin the Qt behaviour the assertions below
    // are built on, so that if a future Qt changes it this suite says so
    // instead of quietly asserting something that stopped being true.
    TextInput {
        id: probePlain
        visible: false
        Accessible.role: Accessible.EditableText
        Accessible.name: "probe-name"
        Accessible.description: "probe-desc"
    }
    TextInput {
        id: probePassword
        visible: false
        Accessible.role: Accessible.EditableText
        Accessible.name: "probe-name"
        Accessible.description: "probe-desc"
        Accessible.passwordEdit: true
    }

    // ── The surface under test: the shipped file ─────────────────────────────
    // Declared, not Loader'd: `theme` and `ctx` are `required property`, and a
    // required property can only be supplied at creation. A Loader would have
    // to assign them in onLoaded, which is too late and fails the component.
    GreetSurface {
        id: surface
        anchors.fill: parent
        theme: stubTheme
        ctx:   stubCtx
    }

    // ── Tree helpers ─────────────────────────────────────────────────────────
    function walk(item, out) {
        if (!item || !item.children) return out
        for (var i = 0; i < item.children.length; i++) {
            var c = item.children[i]
            out.push(c)
            walk(c, out)
        }
        return out
    }

    function all() { return walk(surface, []) }

    function named(objName) {
        var a = all()
        for (var i = 0; i < a.length; i++)
            if (a[i].objectName === objName) return a[i]
        return null
    }

    // Every item carrying a non-empty objectName, in tree order.
    function namedItems() {
        var a = all(), out = []
        for (var i = 0; i < a.length; i++)
            if (a[i].objectName && a[i].objectName.length > 0) out.push(a[i])
        return out
    }

    // The item that actually holds focus: activeFocus is true all the way up
    // the ancestor chain, so the focus OWNER is the deepest such item.
    function focusedItem() {
        var a = all(), deepest = null
        for (var i = 0; i < a.length; i++) {
            var c = a[i]
            if (!c.activeFocus) continue
            var childHasIt = false
            for (var j = 0; j < c.children.length; j++)
                if (c.children[j].activeFocus) { childHasIt = true; break }
            if (!childHasIt) deepest = c
        }
        return deepest
    }

    function focusedName() {
        var f = focusedItem()
        return f ? (f.objectName || "<unnamed>") : "<none>"
    }

    TestCase {
        id: tc
        name: "greet-a11y"
        when: windowShown

        // ── 000: the fixture is real ─────────────────────────────────────────
        // TestCase sets visible:false on itself, so a surface parented to the
        // TestCase would be invisible, fail hit-testing and receive no key
        // event — and every "focus moved" assertion would then be a vacuous
        // pass. The surface is a sibling here, and this proves it.
        function test_000_fixture_is_live() {
            verify(surface !== null,
                   "GreetSurface.qml did not load; every assertion below would be vacuous")
            verify(surface.visible, "the surface is not visible")
            verify(fixture.all().length > 20,
                   "only " + fixture.all().length + " items in the surface tree")
        }

        // The two text fields are the subject of most of what follows. If the
        // surface ever stops having exactly two, the suite must fail rather
        // than silently assert over whatever is left.
        function test_001_two_text_fields() {
            var a = fixture.all(), inputs = 0
            for (var i = 0; i < a.length; i++)
                if (a[i] instanceof TextInput) inputs++
            compare(inputs, 2, "expected exactly two TextInput fields in the greeter")
        }

        // ── Accessible names: what a screen reader announces ─────────────────
        function test_010_username_has_accessible_name() {
            var it = fixture.named("greetUsernameField")
            verify(it !== null, "no item named greetUsernameField")
            verify(it.Accessible.name.length > 0,
                   "the username field has no accessible name; a screen reader announces an unlabelled edit box")
        }

        // ── The premise, pinned ──────────────────────────────────────────────
        //
        // Qt makes `Accessible.name` and `Accessible.passwordEdit` mutually
        // exclusive. From qquickaccessibleattached_p.h:91-93 —
        //
        //     QString name() const {
        //         if (m_state.passwordEdit)
        //             return QString();
        //
        // — so an item flagged as a password edit reports NO accessible name at
        // all, whatever the QML says. `description()` (same header, :111) has no
        // such guard.
        //
        // That is why the password field below is asserted on its DESCRIPTION
        // rather than its name. Without this probe that assertion looks like a
        // sloppy choice; with it, it is the only choice Qt leaves, and the day
        // Qt changes its mind this test fails and the greeter can go back to
        // carrying the label where it belongs.
        function test_002_qt_suppresses_name_for_password_edits() {
            compare(probePlain.Accessible.name, "probe-name",
                    "a plain editable text reports the name it was given")
            compare(probePassword.Accessible.name, "",
                    "Qt no longer suppresses the accessible name of a password edit — " +
                    "move the greeter's label back from description to name")
            compare(probePassword.Accessible.description, "probe-desc",
                    "description is suppressed too; the password field has no label at all")
        }

        // Given the above, this is what a reader can actually be told about the
        // field. Asserted on content, not merely on non-emptiness: a
        // description that says nothing about what the field is would satisfy a
        // length check and help nobody.
        function test_011_password_is_identified_to_a_reader() {
            var it = fixture.named("greetPasswordField")
            verify(it !== null, "no item named greetPasswordField")
            verify(it.Accessible.description.toLowerCase().indexOf("password") >= 0,
                   "the password field's accessible description is '" +
                   it.Accessible.description + "', which does not say it is the password")
        }

        // A role of NoRole (0) is what an item with no declared role reports,
        // and it is the difference between "edit box, Password" and an
        // anonymous element the reader skips.
        function test_012_fields_declare_roles() {
            var u = fixture.named("greetUsernameField")
            var p = fixture.named("greetPasswordField")
            verify(u !== null && p !== null, "a named field is missing")
            compare(u.Accessible.role, Accessible.EditableText,
                    "username field role")
            compare(p.Accessible.role, Accessible.EditableText,
                    "password field role")
        }

        // Without this a reader announces the masked bullets as the value.
        function test_013_password_is_marked_a_password() {
            var p = fixture.named("greetPasswordField")
            verify(p !== null, "no item named greetPasswordField")
            verify(p.Accessible.passwordEdit,
                   "the password field is not flagged as a password edit")
        }

        // The session picker decides which desktop the login starts. It is a
        // control, so it has to announce itself as one.
        function test_014_session_controls_have_names() {
            var names = ["greetSessionPrev", "greetSessionNext"]
            for (var i = 0; i < names.length; i++) {
                var it = fixture.named(names[i])
                verify(it !== null, "no item named " + names[i])
                verify(it.Accessible.name.length > 0,
                       names[i] + " has no accessible name")
                compare(it.Accessible.role, Accessible.Button, names[i] + " role")
            }
        }

        // The session NAME is the only thing that says which desktop is
        // selected. Unannounced, the two buttons change something invisible.
        function test_015_session_name_is_announced() {
            var it = fixture.named("greetSessionName")
            verify(it !== null, "no item named greetSessionName")
            verify(it.Accessible.name.indexOf(stubCtx.sessionName) >= 0,
                   "the announced session name does not contain '" + stubCtx.sessionName +
                   "'; got '" + it.Accessible.name + "'")
        }

        // ── Keyboard-only login: the real Tab chain ──────────────────────────
        // Pressed, not read off KeyNavigation. Each press is followed by a
        // read of who actually holds focus.
        function tabChain(steps) {
            var seen = []
            for (var i = 0; i < steps; i++) {
                seen.push(fixture.focusedName())
                keyClick(Qt.Key_Tab)
            }
            return seen
        }

        function test_020_every_control_is_reachable_by_tab() {
            // Start from a known place rather than from whatever focusInitial()
            // left behind, so the chain is a property of the surface and not of
            // the order the tests happened to run in.
            var u = fixture.named("greetUsernameField")
            verify(u !== null, "no item named greetUsernameField")
            u.forceActiveFocus()

            var chain = tc.tabChain(5)

            // The expected ring, in order. Asserted as a whole: a control that
            // is dropped shortens the ring and fails here, and one that is
            // inserted in the wrong place fails here too.
            //
            // The layout indicator sits between the username and the password
            // on purpose — see test_032. That ordering is the focus-order half
            // of "keyboard layout before password".
            var want = ["greetUsernameField", "greetLayoutButton",
                        "greetPasswordField", "greetSessionPrev", "greetSessionNext"]
            compare(chain.length, want.length, "tab chain length")
            for (var i = 0; i < want.length; i++)
                compare(chain[i], want[i], "tab stop " + i)
        }

        // The ring walk above is driven by explicit KeyNavigation links, and
        // Qt honours those even for an item whose activeFocusOnTab is false —
        // so the walk alone cannot tell whether these are really tab stops.
        // Flipping that flag on a control was a mutation this suite SURVIVED
        // until this assertion existed. Read off the live objects, not the
        // source.
        function test_023_every_ring_member_is_a_tab_stop() {
            var names = ["greetUsernameField", "greetLayoutButton",
                         "greetPasswordField", "greetSessionPrev", "greetSessionNext"]
            for (var i = 0; i < names.length; i++) {
                var it = fixture.named(names[i])
                verify(it !== null, "no item named " + names[i])
                verify(it.activeFocusOnTab,
                       names[i] + " is reachable only through an explicit " +
                       "KeyNavigation link, not as a tab stop of its own")
            }
        }

        function test_021_tab_ring_closes() {
            var u = fixture.named("greetUsernameField")
            verify(u !== null, "no item named greetUsernameField")
            u.forceActiveFocus()
            for (var i = 0; i < 5; i++) keyClick(Qt.Key_Tab)
            compare(fixture.focusedName(), "greetUsernameField",
                    "five tabs from the username field must come back to it")
        }

        // A control that takes focus and does nothing on Space or Enter is not
        // keyboard-operable, however well it is labelled.
        function test_022_session_buttons_operate_from_the_keyboard() {
            var next = fixture.named("greetSessionNext")
            verify(next !== null, "no item named greetSessionNext")
            var before = stubCtx.cycleCount
            next.forceActiveFocus()
            keyClick(Qt.Key_Space)
            compare(stubCtx.cycleCount, before + 1,
                    "Space on the focused session-next button did not cycle the session")
        }

        // ── The keyboard layout, before the password is typed (P2-004) ───────
        // A user on AZERTY whose password contains a symbol cannot type it on a
        // US map, and the login screen is where that is discovered. So the
        // layout has to be VISIBLE before the password field is used, and
        // readable by a screen reader.
        function test_030_layout_indicator_exists_and_is_visible() {
            var it = fixture.named("greetLayoutButton")
            verify(it !== null, "no item named greetLayoutButton")
            verify(it.visible, "the layout indicator is not visible")
        }

        // Bound to the context, not to a literal: drive the context and the
        // indicator has to follow, which a hardcoded "us" cannot do.
        function test_031_layout_indicator_follows_the_session() {
            var it = fixture.named("greetLayoutButton")
            verify(it !== null, "no item named greetLayoutButton")
            stubCtx.layoutName = "fr"
            verify(it.Accessible.name.toLowerCase().indexOf("fr") >= 0,
                   "the layout indicator still announces '" + it.Accessible.name +
                   "' after the session layout became fr")
            stubCtx.layoutName = "us"
        }

        // It must come before the password field on screen. "Before" is a
        // position, and this reads the mapped geometry rather than the source
        // order, because a Column reorders nothing but an anchor can.
        function test_032_layout_is_above_the_password_field() {
            var lay = fixture.named("greetLayoutButton")
            var pw  = fixture.named("greetPasswordField")
            verify(lay !== null && pw !== null, "a named control is missing")
            var lp = lay.mapToItem(surface, 0, 0)
            var pp = pw.mapToItem(surface, 0, 0)
            verify(lp.y < pp.y,
                   "the layout indicator sits at y=" + lp.y +
                   ", below the password field at y=" + pp.y)
        }

        // With more than one layout configured it has to be switchable without
        // a mouse — and a single-layout machine must not offer a switch that
        // does nothing.
        function test_033_layout_switches_from_the_keyboard() {
            var it = fixture.named("greetLayoutButton")
            verify(it !== null, "no item named greetLayoutButton")
            stubCtx.layouts = ["us", "fr"]
            var before = stubCtx.layoutCycles
            it.forceActiveFocus()
            keyClick(Qt.Key_Space)
            compare(stubCtx.layoutCycles, before + 1,
                    "Space on the focused layout indicator did not cycle the layout")
        }

        // The other half of the same claim: a machine with one layout must not
        // offer a switch that does nothing. Without this assertion the
        // indicator could satisfy test_033 by cycling unconditionally, and a
        // single-layout user would press Space on a control that answers by
        // changing nothing — which is the failure mode the surface's own
        // comment says it exists to avoid.
        function test_034_single_layout_offers_no_switch() {
            var it = fixture.named("greetLayoutButton")
            verify(it !== null, "no item named greetLayoutButton")
            stubCtx.layouts = ["us"]
            var before = stubCtx.layoutCycles
            it.forceActiveFocus()
            keyClick(Qt.Key_Space)
            compare(stubCtx.layoutCycles, before,
                    "Space cycled the layout on a machine with only one configured")
        }

        // ── Status: the things the surface says only in colour or in glyphs ──
        // Caps Lock on and a failed attempt are both announced today by a
        // shake, a border colour and a Nerd Font glyph. None of the three
        // reaches a screen reader.
        function test_040_error_text_is_announced() {
            var it = fixture.named("greetStatusLine")
            verify(it !== null, "no item named greetStatusLine")
            stubCtx.errorText = "Authentication failed"
            stubCtx.hasError  = true
            verify(it.Accessible.name.indexOf("Authentication failed") >= 0,
                   "the status line announces '" + it.Accessible.name +
                   "' while the context is reporting an error")
            compare(it.Accessible.role, Accessible.AlertMessage,
                    "the status line must be an alert, or a reader will not interrupt for it")
            stubCtx.hasError  = false
            stubCtx.errorText = ""
        }

        function test_041_caps_lock_is_announced() {
            var pw = fixture.named("greetPasswordField")
            var st = fixture.named("greetStatusLine")
            verify(pw !== null && st !== null, "a named control is missing")
            pw.forceActiveFocus()
            // A capital typed with no Shift is the surface's own caps-lock
            // heuristic; this drives it exactly as a user would.
            keyClick("A")
            verify(st.Accessible.name.toLowerCase().indexOf("caps") >= 0,
                   "caps lock is on and the status line announces '" +
                   st.Accessible.name + "'")
            keyClick(Qt.Key_Escape)
        }
    }
}
