package com.apexos.remote.core.agent

/**
 * Voice, clipboard and file handoff from the phone to an agent (P1-059).
 *
 * Three of P1-059's four criteria are about pushing something the person is
 * holding — a sentence they said, something they copied, a photo they took —
 * into an agent that is already running. One verb carries all of it that this
 * protocol can carry, and one of the three has no transport at all. This file
 * is where each of those is decided, and where the one that cannot be done
 * says so rather than being approximated.
 *
 * ## The verb, and the fact that it appends nothing
 *
 * `Request::Input { id, data }` writes `data` to the session's PTY master with
 * `session::write_input`, which the protocol documents as writing it "verbatim
 * and no byte is added". Two consequences run through everything below.
 *
 * **Bytes on a PTY are keystrokes.** There is no field in a terminal for "this
 * came from a phone". A `\n` in the middle of pasted text is not text; it is
 * the return key, pressed at that point, and everything after it is typed into
 * whatever the agent asked next. An `ESC` is not a character; it is the first
 * byte of whatever sequence follows it. `Agentd.escape` carries control bytes
 * through the JSON faithfully, so an escaped 0x1b arrives as one 0x1b byte — the
 * escaping is correct and is precisely why the hazard survives to the terminal.
 *
 * **Nothing here submits.** `apex agent input <id> <text>` takes a `--submit`
 * flag and the desktop's own push-to-talk does not pass it:
 * `PushToTalkService.qml` builds `["apex", "agent", "input", id, text]`, full
 * stop. The words land in the agent's input line and a human presses Enter.
 * That is not a limitation being inherited; it is the right rule and the
 * reason is [Voice]'s: speech recognition is wrong often enough that
 * submitting its guess is submitting an instruction nobody read.
 * [com.apexos.remote.core.agent.Reply.bytes] appends CR because a person who
 * typed a sentence and pressed Send meant to send it. A transcript is not that,
 * and it does not go through [Reply].
 */
object Handoff {

    // ── Push to talk ────────────────────────────────────────────────────────

    /**
     * Speaking to a named agent.
     *
     * The rules are the desktop's, from `apex-shell/src/services/pushtotalk.js`,
     * and they are reproduced rather than reinvented so that the same feature
     * does not mean two things on two clients. Two of them are the whole design:
     *
     * 1. **The target is resolved before the microphone opens.** A refusal keeps
     *    the phase at [Phase.IDLE]. There is no reachable path on which the
     *    phase is [Phase.RECORDING] and the target is null — "a recording with
     *    nowhere to go is a hot microphone with a reason to feel fine about
     *    itself".
     * 2. **The target is frozen when recording starts.** A person talking to a
     *    computer looks away from it; a route re-resolved at delivery would
     *    send the words wherever the phone had drifted to.
     *
     * And one is inherited with its reason examined rather than copied:
     *
     * 3. **[MAX_MS] bounds an open microphone.** The desktop needs the cap
     *    because push-to-talk there is a *toggle* — niri fires nothing on key
     *    release, so hold-to-talk is unimplementable on one of the three
     *    supported compositors and one semantic everywhere had to be the
     *    toggle. **That reasoning does not transfer.** A touch surface reports
     *    a lift, so this app can hold-to-talk and the microphone shuts when the
     *    finger leaves. The cap is kept anyway, at the same ninety seconds,
     *    because a lift that is never delivered — the gesture cancelled by a
     *    system dialog, the app backgrounded mid-word — is exactly the failure
     *    the cap was written for, and because two clients whose microphones
     *    stop at different times is a difference nobody can hold in their head.
     *
     * This reducer is agnostic about which gesture produced [stop]: a second
     * tap and a finger lift are the same event. What it is not agnostic about
     * is that something must have produced it.
     */
    object Voice {

        /**
         * Longest single recording, in milliseconds.
         *
         * Ninety seconds, agreeing with `pushtotalk.js`'s `MAX_MS`. Longer than
         * anything a person says in one breath, short enough that a microphone
         * left open is an embarrassment and not an incident.
         */
        const val MAX_MS: Long = 90_000L

        /** Where push-to-talk is in its cycle. */
        enum class Phase {
            IDLE,

            /** The microphone is open. The target is already decided. */
            RECORDING,

            /** Audio captured, words not yet out. Not interruptible. */
            TRANSCRIBING,

            /** Words in hand, `input` in flight. Not interruptible. */
            DELIVERING,

            /** Something refused, and [State.error] says what. */
            ERROR,
        }

        /**
         * The whole of what push-to-talk knows.
         *
         * [target] is null in exactly two phases — [Phase.IDLE] and
         * [Phase.ERROR] — and `targetIsHeldWhileTheMicrophoneIsOpen` asserts
         * it, because that invariant is the feature.
         */
        data class State(
            val phase: Phase = Phase.IDLE,
            val target: Reply.Target? = null,
            /** What the user is told they are talking to. Display only. */
            val label: String = "",
            /** The transcript, once there is one. */
            val text: String = "",
            /** When [Phase.RECORDING] began, for [remainingMs]. */
            val startedAt: Long = 0L,
            val error: String = "",
        )

        /**
         * Why the microphone did not open.
         *
         * Each is a different answer, and a screen that collapsed them into
         * "push-to-talk did nothing" would be the report this exists to avoid.
         */
        enum class Refusal(val message: String) {
            /** Nothing is running on this machine to talk to. */
            NO_SESSIONS(
                "There is no agent running on this machine to talk to. Start one first.",
            ),

            /**
             * The user pinned a target and it has gone.
             *
             * Deliberately not a fall-through to something else that happens to
             * be running: they named a destination. `pushtotalk.js` refuses the
             * same case for the same reason.
             */
            PINNED_GONE(
                "The agent you chose to talk to is no longer running. Pick another one.",
            ),

            /** Several are running and none was chosen. */
            AMBIGUOUS(
                "Several agents are running. Choose the one to talk to — push-to-talk will " +
                    "not guess which of them should hear this.",
            ),

            /** The microphone permission has not been granted. */
            NO_MICROPHONE(
                "APEX Remote has not been given the microphone. Grant it in Android's " +
                    "settings, or type instead.",
            ),

            /** No speech recogniser on this device. */
            NO_RECOGNISER(
                "This phone has no speech recognition available offline or otherwise, so " +
                    "there is nothing to turn speech into text. Type instead.",
            ),
        }

        /** A target that was chosen, or why none was. */
        data class Route(
            val target: Reply.Target?,
            val label: String,
            val refusal: Refusal?,
        )

        /**
         * Which agent the words would go to, resolved **before** anything opens
         * the microphone.
         *
         * @param machine the device id the phone is connected to now.
         * @param live the sessions from the most recent `list`.
         * @param pinned a session id the user chose, or null.
         *
         * The order is the desktop's: a pinned target outranks everything,
         * because it is a choice and the alternative is a guess. With nothing
         * pinned, exactly one running agent is unambiguous and more than one is
         * not — this app has no notion of "focused", so where the desktop falls
         * back to the focused session it refuses instead. Guessing would be the
         * worse failure: these words become an instruction.
         */
        fun route(
            machine: String,
            live: List<AgentSession>,
            pinned: Int? = null,
        ): Route {
            val running = live.filter { !it.isTerminal }
            if (pinned != null) {
                val found = running.firstOrNull { it.id == pinned }
                    ?: return Route(null, "", Refusal.PINNED_GONE)
                return Route(found.toTarget(machine), found.routeLabel, null)
            }
            return when (running.size) {
                0 -> Route(null, "", Refusal.NO_SESSIONS)
                1 -> running[0].let { Route(it.toTarget(machine), it.routeLabel, null) }
                else -> Route(null, "", Refusal.AMBIGUOUS)
            }
        }

        /**
         * Open the microphone, or refuse and stay shut.
         *
         * [route] is called by the caller and its answer passed in, so that a
         * screen can show the destination before a finger goes down; this
         * function re-checks it rather than trusting that it was checked,
         * because the invariant is worth more than the duplicated branch.
         */
        fun start(state: State, route: Route, now: Long): State {
            // Not interruptible, and a second press while words are in flight
            // must not start a new recording on top of them.
            if (state.phase == Phase.TRANSCRIBING || state.phase == Phase.DELIVERING) return state
            val target = route.target
                ?: return State(phase = Phase.ERROR, error = (route.refusal ?: Refusal.NO_SESSIONS).message)
            return State(
                phase = Phase.RECORDING,
                target = target,
                label = route.label,
                startedAt = now,
            )
        }

        /**
         * The finger came up, or the second tap arrived, or [MAX_MS] ran out.
         *
         * All three are one event. The target is carried forward untouched —
         * that is what "frozen" means.
         */
        fun stop(state: State): State =
            if (state.phase != Phase.RECORDING) state
            else state.copy(phase = Phase.TRANSCRIBING)

        /**
         * Whether an open microphone has outstayed [MAX_MS].
         *
         * Checked by the caller on a timer; a reducer cannot see a clock.
         */
        fun expired(state: State, now: Long): Boolean =
            state.phase == Phase.RECORDING && now - state.startedAt >= MAX_MS

        /** How long an open microphone has left, for the indicator. */
        fun remainingMs(state: State, now: Long): Long =
            if (state.phase != Phase.RECORDING) 0L
            else (MAX_MS - (now - state.startedAt)).coerceAtLeast(0L)

        /**
         * The recogniser answered.
         *
         * An empty or blank transcript is not a delivery. Sending it would put
         * nothing in the agent's input line while the phone said it had spoken,
         * and the user would never learn that the room was too loud.
         */
        fun transcribed(state: State, text: String): State {
            if (state.phase != Phase.TRANSCRIBING) return state
            if (text.isBlank()) {
                return State(
                    phase = Phase.ERROR,
                    error = "Nothing was heard, so nothing was sent.",
                )
            }
            return state.copy(phase = Phase.DELIVERING, text = text)
        }

        /** `input` was accepted. */
        fun delivered(state: State): State =
            if (state.phase != Phase.DELIVERING) state else State()

        /**
         * Something refused.
         *
         * Accepted from any phase: an error can arrive from the recorder, the
         * recogniser or the socket, and a guard that only fired on one of them
         * would strand the microphone open in a phase nothing could leave.
         */
        fun failed(state: State, why: String): State =
            State(phase = Phase.ERROR, error = why.ifBlank { "Push-to-talk failed." })

        /** Back to idle from [Phase.ERROR], when the user has read it. */
        fun dismiss(state: State): State =
            if (state.phase == Phase.ERROR) State() else state

        /** Whether the indicator should show an open microphone. */
        fun micOpen(state: State): Boolean = state.phase == Phase.RECORDING

        /**
         * What goes on the wire for a finished transcript.
         *
         * **No terminator.** This is the one place the difference between this
         * and [Reply.bytes] is decided, and it is deliberate: `apex agent
         * input` takes `--submit` and the desktop's push-to-talk does not pass
         * it. A transcript is a machine's guess at what a person said, and a
         * guess that submits itself is an instruction nobody read. It is staged
         * in the agent's input line, visible, and a human presses Enter — the
         * same rule `apex_agent_core::inject` applies to a handed-over file,
         * for the same reason.
         *
         * The trailing space matters and is not padding: the words are staged
         * where a person will keep typing, and a caret jammed against the last
         * word is the difference between adding a clause and editing one.
         */
        fun payload(text: String): String = text.trim() + " "
    }

    // ── Clipboard ───────────────────────────────────────────────────────────

    /**
     * Sending what is on the phone's clipboard to an agent.
     *
     * ## Send is real. Receive is not, and this says so instead of pretending.
     *
     * There is **no verb on this socket that reads the machine's clipboard**.
     * The full wire vocabulary is 28 verbs and none of them is a clipboard
     * verb. `apex send --clipboard` is real and is a different feature wearing
     * a similar name: `apexd/apex/src/dispatch.rs` shells out to `wl-paste`,
     * ssh's the bytes to another **Linux host in the §20 registry** and runs
     * `wl-copy` there. A phone is not an ssh destination running `wl-copy`, so
     * that path is not reachable from here and adding it would mean a verb, a
     * daemon-side clipboard reader and a Wayland session to read it from.
     *
     * What exists in the receive direction is the terminal's own copy: the
     * session screen selects and copies what is on it, which covers "get that
     * command off the machine" and does not cover "get what the machine
     * copied". P1-059's third criterion is therefore half met, and it is
     * recorded that way.
     *
     * ## Why a paste is not a reply
     *
     * `Reply.bytes` trims the ends and appends CR, which is right for a
     * sentence someone typed into a box. A clipboard is not that. It holds
     * whatever was last copied — a stack trace, a URL, a chunk of a file — and
     * `Input` writes it to a PTY **verbatim**. Every interior newline is a
     * press of the return key at that point, so a three-line paste submits the
     * first line and types the other two into whatever the agent asked next. An
     * `ESC` is the start of a control sequence, and TUIs act on those.
     *
     * The daemon does not help here and could not: `inject` wraps its path in
     * bracketed-paste markers when `OutputScanner` has seen the application set
     * DECSET 2004, because the daemon *is* the terminal and knows. `input`
     * brackets nothing, and a phone has no way to learn whether the mode is on.
     *
     * So a paste is classified before it is sent and the user is told what will
     * happen. Nothing is silently stripped: text that came off a clipboard is
     * text somebody chose, and a send that quietly delivered something else
     * would be worse than one that refused.
     */
    object Clipboard {

        /**
         * What typing this clipboard into a live PTY would do.
         *
         * @param lines how many lines the content would be typed as. One means
         *   the whole paste lands on the input line.
         * @param submits true when an interior CR or LF would press return
         *   before the end of the content.
         * @param controlBytes the C0/DEL bytes present other than the line
         *   breaks already counted — the ones a TUI acts on rather than shows.
         *   Reported as code points so a message can name them.
         * @param truncated whether [MAX_CHARS] would clip it.
         */
        data class Shape(
            val lines: Int,
            val submits: Boolean,
            val controlBytes: List<Int>,
            val truncated: Boolean,
        ) {
            /** Nothing here needs explaining before it is sent. */
            val isPlain: Boolean get() = !submits && controlBytes.isEmpty() && !truncated
        }

        /**
         * Longest clipboard this app will send in one `input`.
         *
         * A control frame on the remote wire carries at most
         * `apex_remote_core::wire::MAX_PAYLOAD` = 65514 bytes and a request is
         * one frame; JSON escaping turns one control byte into six characters.
         * 16384 characters is comfortably inside that for any encoding, and it is far
         * more than anybody pastes into a prompt on purpose. Above it the user
         * is told rather than having the tail silently dropped — or, worse, the
         * frame refused with a wire error they cannot act on.
         */
        const val MAX_CHARS: Int = 16_384

        /** Classify a clipboard without sending it. */
        fun inspect(text: String): Shape {
            val submits = text.dropLast(1).any { it == '\n' || it == '\r' }
            val control = text
                .filter { (it.code < 0x20 && it != '\n' && it != '\r' && it != '\t') || it.code == 0x7F }
                .map { it.code }
                .distinct()
                .sorted()
            // Counted the way a terminal counts them: CRLF is one press of
            // return, not two, and a trailing break does not open a line that
            // has nothing on it.
            val normalised = text.replace("\r\n", "\n").replace('\r', '\n').trimEnd('\n')
            val lines = if (normalised.isEmpty()) 0 else normalised.count { it == '\n' } + 1
            return Shape(
                lines = lines,
                submits = submits,
                controlBytes = control,
                truncated = text.length > MAX_CHARS,
            )
        }

        /**
         * The two things a user may choose to do with a paste that is not
         * plain.
         *
         * Named rather than left to a screen, because the safe one has to be
         * the default and a screen that got that backwards would be typing
         * somebody's stack trace into an agent as a series of commands.
         */
        enum class Choice {
            /**
             * The first line only, staged and not submitted.
             *
             * The default for anything [Shape.isPlain] is false about. It is
             * the answer that cannot do something the user did not ask for.
             */
            FIRST_LINE,

            /**
             * All of it, exactly as copied, control bytes included.
             *
             * Only after the user has been told what it will do. This is not
             * hidden behind a flag because it is wrong — a deliberate
             * multi-line paste into an agent that is asking for a here-doc is a
             * real thing to want — but it is never what a tap lands on by
             * accident.
             */
            EVERYTHING,
        }

        /**
         * The bytes for a clipboard send.
         *
         * Never terminated, for [Voice.payload]'s reason and one more: the
         * content already contains whatever line breaks the user copied, so
         * appending another would submit a paste the user was still looking at.
         */
        fun payload(text: String, choice: Choice): String {
            val clipped = text.take(MAX_CHARS)
            return when (choice) {
                Choice.EVERYTHING -> clipped
                Choice.FIRST_LINE ->
                    clipped.replace("\r\n", "\n").replace('\r', '\n').substringBefore('\n')
            }
        }
    }

    // ── Photo, screenshot and file ──────────────────────────────────────────

    /**
     * P1-059's second criterion, and why no code here meets it.
     *
     * > Photo/screenshot/file upload lands in an explicit permitted
     * > project/temp path and inserts a safe path into the agent session.
     *
     * The destination half of that is **built and working on the desktop**.
     * `apex_agent_core::inject` copies a file into the session's own scratch
     * `inbox/`, names the copy from an allowlist of 64 characters so no byte of
     * the user's filename reaches the PTY unreduced, types the path, submits
     * nothing, and mirrors every handover to `systemd-journald` where the
     * session cannot rewrite it. P1-035 landed all of it. A phone does not need
     * a new destination; it needs a way to get the bytes there.
     *
     * **There is no such way, and the arithmetic says how far off it is.** The
     * wire vocabulary has no verb that carries file content. The nearest
     * relative is `Request::Inject { id, source }`, whose `source` is a path on
     * the **host** read with the daemon's own access — which is why this round
     * gated it to local origins after finding a paired phone could have named
     * `~/.ssh/id_ed25519` with it.
     *
     * Carrying bytes in a request instead would mean base64 in JSON inside one
     * `Frame::Control`, and `apex_remote_core::wire::MAX_PAYLOAD` is 65514
     * bytes. Base64 costs four bytes for three, the JSON envelope takes its
     * share, and what is left is about **48 KB of file**. A phone screenshot is
     * 100 KB to 2 MB. A verb built to that shape would carry a log tail and
     * refuse a screenshot — the feature's own name — which is the mistake this
     * app has already made once, building a request nothing could parse.
     *
     * Doing it honestly is a chunked transfer: begin, chunk, end, with staging
     * state in the daemon, a size cap, a timeout, cleanup for an upload that is
     * abandoned halfway, and the origin gate above applied to the destination
     * rather than to the source. That is a protocol unit and it is named as one
     * rather than started at the end of an Android round.
     *
     * ## What this app does instead
     *
     * It does not offer the button. A picker that opened, took a photo and then
     * said the machine could not receive it would be worse than no picker: the
     * user has already granted access to that image by the time they learn.
     * [WHY] is what the Session screen shows where the control would be, in the
     * app's own words, so the absence is explained where somebody looks for it.
     */
    object Files {

        /** Shown where the attach control would be. */
        const val WHY: String =
            "Sending a photo or a file from this phone needs something APEX does not have yet: " +
                "a way to carry file content over this connection. The machine can already " +
                "hand a file to a running agent — `apex agent send` does it — but only for a " +
                "file that is already on the machine, and only from a terminal there."

        /**
         * The rule this app follows about the phone's own storage, which is
         * P1-059's fourth criterion.
         *
         * `ACTION_PICK_IMAGES` (androidx's `PickVisualMedia`) returns a URI for
         * the one item the user chose and needs **no permission at all** — not
         * `READ_MEDIA_IMAGES`, not `READ_EXTERNAL_STORAGE`. Asking for either
         * would give the app the whole library for the life of the grant, which
         * is exactly the "automatic access beyond Android-granted selections"
         * the criterion forbids, and it would do so to no purpose since the
         * picker already returns what was chosen.
         *
         * So the manifest declares neither, and `ManifestPermissionsTest`
         * asserts it. That test is worth having *before* there is a picker
         * rather than after: the moment a transport exists, the obvious way to
         * read the file is a permission, and the guard is what makes the
         * cheaper right answer the one that compiles.
         */
        val FORBIDDEN_PERMISSIONS: List<String> = listOf(
            "android.permission.READ_EXTERNAL_STORAGE",
            "android.permission.WRITE_EXTERNAL_STORAGE",
            "android.permission.READ_MEDIA_IMAGES",
            "android.permission.READ_MEDIA_VIDEO",
            "android.permission.READ_MEDIA_VISUAL_USER_SELECTED",
            "android.permission.MANAGE_EXTERNAL_STORAGE",
            "android.permission.ACCESS_MEDIA_LOCATION",
        )
    }
}

/** The three values that make a session itself, for [Reply.check]. */
private fun AgentSession.toTarget(machine: String): Reply.Target =
    Reply.Target(machine = machine, session = id, started = started)

/** What the user is told they are about to talk to. */
private val AgentSession.routeLabel: String
    get() = listOfNotNull(
        agentName.takeIf { it.isNotBlank() },
        where.takeIf { it.isNotBlank() }?.substringAfterLast('/'),
    ).joinToString(" in ").ifBlank { "session $id" }
