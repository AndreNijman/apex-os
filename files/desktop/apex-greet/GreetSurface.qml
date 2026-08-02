import QtQuick
import QtQuick.Effects

// ─────────────────────────────────────────────────────────────
// GreetSurface — the per-output visual.
//
// A pixel-faithful port of the Brain_Shell Lockscreen surface:
// gradient fallback + blurred/dimmed wallpaper, the big clock block,
// and the pill auth card (username + password). Binds to a shared
// GreetContext (`ctx`) and the inlined palette (`theme`) supplied by
// shell.qml.
// ─────────────────────────────────────────────────────────────

Item {
    id: root
    required property var theme
    required property var ctx

    // Wallpaper is a system path (not shipped with the greeter); a
    // missing file falls back to the gradient below. Resolved by the
    // shared context: the last user's own published wallpaper when
    // there is one, the shipped default otherwise. It used to be this
    // literal default, which is why the login screen never followed the
    // wallpaper you picked in APEX Shell — see GreetContext.
    readonly property url wallpaper: root.ctx.wallpaper

    property real shakeOffset: 0
    property bool capsOn:      false

    focus: true
    // Any stray keystroke lands in the password field.
    Keys.forwardTo: [passwordInput]

    // Re-shake + clear the field whenever an attempt fails.
    Connections {
        target: root.ctx
        function onFailed() {
            passwordInput.text = ""
            shakeAnim.restart()
        }
    }

    // Keep the username field in sync when the last-user file resolves
    // asynchronously after this surface has already loaded.
    Connections {
        target: root.ctx
        function onUsernameChanged() {
            if (!usernameInput.activeFocus) usernameInput.text = root.ctx.username
        }
    }

    // ── Background: gradient fallback ──────────────────────────────
    // Always present so an empty / broken wallpaper path can never
    // leave a blank surface.
    Rectangle {
        anchors.fill: parent
        gradient: Gradient {
            orientation: Gradient.Vertical
            GradientStop { position: 0.0; color: Qt.darker(root.theme.background, 1.15) }
            GradientStop {
                position: 1.0
                color: Qt.rgba(root.theme.active.r, root.theme.active.g, root.theme.active.b, 1.0)
            }
        }
    }

    // Wallpaper texture (hidden; fed into the blur effect).
    Image {
        id: wallImg
        anchors.fill: parent
        source:       root.wallpaper
        fillMode:     Image.PreserveAspectCrop
        asynchronous: true
        cache:        true
        visible:      false
    }

    // Blurred + dimmed wallpaper; hidden if the image fails to load,
    // revealing the gradient underneath.
    MultiEffect {
        anchors.fill: parent
        source:      wallImg
        visible:     wallImg.status === Image.Ready
        blurEnabled: true
        blur:        1.0
        blurMax:     48
        brightness: -0.30
        saturation: -0.10
    }

    // Extra scrim for legibility.
    Rectangle {
        anchors.fill: parent
        color: Qt.rgba(0, 0, 0, 0.35)
    }

    // Clicking anywhere re-focuses the password field.
    MouseArea {
        anchors.fill: parent
        onClicked: passwordInput.forceActiveFocus()
    }

    // ── Clock + date ───────────────────────────────────────────────
    Column {
        id: clockBlock
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.bottom:           card.top
        anchors.bottomMargin:     56
        spacing: 4

        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            text:           root.ctx.timeText
            color:          root.theme.text
            font.family:    root.theme.fontFamily
            font.pixelSize: 120
            font.bold:      true
        }
        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            text:           root.ctx.dateText
            color:          root.theme.subtext
            font.family:    root.theme.fontFamily
            font.pixelSize: 22
        }
    }

    // ── Auth card ──────────────────────────────────────────────────
    Column {
        id: card
        anchors.centerIn: parent
        anchors.verticalCenterOffset: 90
        spacing: 14
        transform: Translate { x: root.shakeOffset }

        // Edition spark logo, centred above the username.
        Image {
            anchors.horizontalCenter: parent.horizontalCenter
            source:            root.theme.logoSource
            sourceSize.height: 84
            fillMode:          Image.PreserveAspectFit
            smooth:            true
            asynchronous:      true
        }

        // Username pill (smaller variant of the password pill).
        Rectangle {
            id: userField
            anchors.horizontalCenter: parent.horizontalCenter
            width:  340
            height: 44
            radius: height / 2
            color:  Qt.rgba(root.theme.background.r, root.theme.background.g, root.theme.background.b, 0.55)
            border.width: 2
            border.color: usernameInput.activeFocus ? root.theme.active : root.theme.border
            Behavior on border.color { ColorAnimation { duration: 140 } }

            // Person glyph
            Text {
                anchors.verticalCenter: parent.verticalCenter
                anchors.left:           parent.left
                anchors.leftMargin:     18
                text:  "󰀄"
                color: root.theme.subtext
                font.family:    root.theme.fontFamily
                font.pixelSize: 16
            }

            TextInput {
                id: usernameInput
                anchors.fill:        parent
                anchors.leftMargin:  44
                anchors.rightMargin: 18
                verticalAlignment:   TextInput.AlignVCenter
                clip:                true
                enabled:             !root.ctx.checking
                color:               root.theme.text
                selectionColor:      root.theme.active
                font.family:         root.theme.fontFamily
                font.pixelSize:      16
                // Tab / Shift+Tab move between the two pills so a
                // keyboard-only login works when no username is prefilled.
                activeFocusOnTab:    true
                KeyNavigation.tab:     passwordInput
                KeyNavigation.backtab: passwordInput
                onTextChanged: root.ctx.username = text
                onAccepted:    passwordInput.forceActiveFocus()
                Component.onCompleted: text = root.ctx.username

                Text {
                    anchors.verticalCenter: parent.verticalCenter
                    anchors.left:           parent.left
                    visible: usernameInput.text.length === 0
                    text:  "Username"
                    color: root.theme.subtext
                    font.family:    usernameInput.font.family
                    font.pixelSize: usernameInput.font.pixelSize
                }
            }
        }

        // Password pill (geometry verbatim from the Lockscreen).
        Rectangle {
            id: field
            anchors.horizontalCenter: parent.horizontalCenter
            width:  340
            height: 52
            radius: height / 2
            color:  Qt.rgba(root.theme.background.r, root.theme.background.g, root.theme.background.b, 0.55)
            border.width: 2
            border.color: root.ctx.hasError
                              ? root.theme.errorColor
                              : (passwordInput.activeFocus ? root.theme.active : root.theme.border)
            Behavior on border.color { ColorAnimation { duration: 140 } }

            // Lock glyph
            Text {
                anchors.verticalCenter: parent.verticalCenter
                anchors.left:           parent.left
                anchors.leftMargin:     18
                text:  "󰌾"
                color: root.theme.subtext
                font.family:    root.theme.fontFamily
                font.pixelSize: 18
            }

            TextInput {
                id: passwordInput
                anchors.fill:        parent
                anchors.leftMargin:  46
                anchors.rightMargin: 52
                verticalAlignment:   TextInput.AlignVCenter
                clip:                true
                enabled:             !root.ctx.checking
                focus:               true
                color:               root.theme.text
                selectionColor:      root.theme.active
                font.family:         root.theme.fontFamily
                font.pixelSize:      18
                echoMode:            TextInput.Password
                passwordCharacter:   "●"
                passwordMaskDelay:   0
                activeFocusOnPress:  true
                // Tab / Shift+Tab step back to the username pill.
                activeFocusOnTab:    true
                KeyNavigation.tab:     usernameInput
                KeyNavigation.backtab: usernameInput

                // Mirror the buffer into shared state (used by Greetd).
                onTextChanged: {
                    root.ctx.password = text
                    if (root.ctx.hasError) root.ctx.hasError = false
                }

                // Enter submits.
                onAccepted: root.ctx.tryAuth()

                Keys.onPressed: function(event) {
                    if (event.key === Qt.Key_Escape) {
                        text = ""
                        event.accepted = true
                        return
                    }
                    if (event.key === Qt.Key_CapsLock) {
                        // Best-effort toggle; the case heuristic below
                        // corrects it as soon as a letter is typed.
                        root.capsOn = !root.capsOn
                        return
                    }
                    // Alt+Left / Alt+Right cycle the session (keyboard
                    // parity with the on-screen ‹ › picker).
                    if ((event.modifiers & Qt.AltModifier) &&
                        (event.key === Qt.Key_Left || event.key === Qt.Key_Right)) {
                        root.ctx.cycleSession(event.key === Qt.Key_Left ? -1 : 1)
                        event.accepted = true
                        return
                    }
                    // Caps-Lock detection via typed-character case.
                    if (event.text.length === 1) {
                        var c = event.text
                        var isLower = (c >= "a" && c <= "z")
                        var isUpper = (c >= "A" && c <= "Z")
                        var shift   = (event.modifiers & Qt.ShiftModifier) !== 0
                        if (isLower || isUpper)
                            root.capsOn = shift ? isLower : isUpper
                    }
                }

                // Placeholder
                Text {
                    anchors.verticalCenter: parent.verticalCenter
                    anchors.left:           parent.left
                    visible: passwordInput.text.length === 0 && !root.ctx.checking
                    text:  "Enter password"
                    color: root.theme.subtext
                    font.family:    passwordInput.font.family
                    font.pixelSize: passwordInput.font.pixelSize
                }
            }

            // Spinner (shown while the auth conversation is in flight).
            Item {
                id: spinner
                anchors.verticalCenter: parent.verticalCenter
                anchors.right:          parent.right
                anchors.rightMargin:    16
                width:  22
                height: 22
                visible: root.ctx.checking

                Rectangle {
                    anchors.fill: parent
                    radius: width / 2
                    color: "transparent"
                    border.width: 3
                    border.color: Qt.rgba(root.theme.active.r, root.theme.active.g, root.theme.active.b, 0.25)
                }
                Rectangle {
                    width: 6; height: 6; radius: 3
                    color: root.theme.active
                    anchors.horizontalCenter: parent.horizontalCenter
                    y: -1
                }
                RotationAnimator on rotation {
                    running: spinner.visible
                    loops:   Animation.Infinite
                    from: 0; to: 360
                    duration: 850
                }
            }
        }

        // Status line — error message or caps-lock warning.
        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            height:  18
            text: root.ctx.hasError ? root.ctx.errorText
                : (root.capsOn ? "󰪛  Caps Lock is on" : "")
            color: root.ctx.hasError ? root.theme.errorColor : root.theme.subtext
            font.family:    root.theme.fontFamily
            font.pixelSize: 14
        }
    }

    // ── Session picker (subtle, bottom-centre) ────────────────────
    // Matches the Lockscreen typography: JetBrainsMono, subtext colour.
    // Rendered as "‹ Session ›"; arrows cycle, hover picks up the accent.
    Row {
        id: sessionPicker
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.bottom:           parent.bottom
        anchors.bottomMargin:     40
        spacing: 14
        visible: root.ctx.sessions.length > 0

        Text {
            id: prevArrow
            anchors.verticalCenter: parent.verticalCenter
            text:  "‹"
            color: prevMouse.containsMouse ? root.theme.active : root.theme.subtext
            font.family:    root.theme.fontFamily
            font.pixelSize: 20
            MouseArea {
                id: prevMouse
                anchors.fill: parent
                hoverEnabled: true
                cursorShape:  Qt.PointingHandCursor
                onClicked:    root.ctx.cycleSession(-1)
            }
        }
        Text {
            anchors.verticalCenter:  parent.verticalCenter
            horizontalAlignment:     Text.AlignHCenter
            width: Math.max(160, implicitWidth)
            text:  root.ctx.sessionName
            color: root.theme.subtext
            font.family:    root.theme.fontFamily
            font.pixelSize: 15
        }
        Text {
            id: nextArrow
            anchors.verticalCenter: parent.verticalCenter
            text:  "›"
            color: nextMouse.containsMouse ? root.theme.active : root.theme.subtext
            font.family:    root.theme.fontFamily
            font.pixelSize: 20
            MouseArea {
                id: nextMouse
                anchors.fill: parent
                hoverEnabled: true
                cursorShape:  Qt.PointingHandCursor
                onClicked:    root.ctx.cycleSession(1)
            }
        }
    }

    // ── Error shake (values verbatim from the Lockscreen) ──────────
    SequentialAnimation {
        id: shakeAnim
        NumberAnimation { target: root; property: "shakeOffset"; from: 0; to:  14; duration: 45 }
        NumberAnimation { target: root; property: "shakeOffset"; to: -14; duration: 45 }
        NumberAnimation { target: root; property: "shakeOffset"; to:  10; duration: 45 }
        NumberAnimation { target: root; property: "shakeOffset"; to: -10; duration: 45 }
        NumberAnimation { target: root; property: "shakeOffset"; to:   6; duration: 45 }
        NumberAnimation { target: root; property: "shakeOffset"; to:   0; duration: 45 }
    }

    // Grab keyboard focus as soon as the surface appears. Start on the
    // username field when nothing is prefilled (fresh boot / no last-user)
    // so a username can be typed without a mouse; otherwise go straight to
    // the password field, matching the returning-user fast path.
    function focusInitial() {
        if (root.ctx.username.length === 0)
            usernameInput.forceActiveFocus()
        else
            passwordInput.forceActiveFocus()
    }
    Component.onCompleted: focusInitial()
    onVisibleChanged: if (visible) focusInitial()
}
