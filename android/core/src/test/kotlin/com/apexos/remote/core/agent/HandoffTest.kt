package com.apexos.remote.core.agent

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertNotNull
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

/**
 * P1-059's rules, where they are decidable without a device.
 *
 * Nothing here opens a microphone, reads a clipboard or touches a picker —
 * those are Android framework calls and this module has no Android on its
 * classpath. What is testable is every rule that decides *what would be sent*,
 * which is the half that can be wrong in a way nobody notices until an agent
 * has been told something the user did not say.
 */
class HandoffTest {

    private fun session(
        id: Int,
        started: Long = 1_000L,
        state: String = AgentStates.WORKING,
        agent: String = "claude",
        project: String? = "/home/andre/Projects/apex",
    ) = AgentSession(
        id = id,
        agent = agent,
        state = state,
        project = project,
        started = started,
    )

    // ── routing: the session on screen, guarded by Reply.check ──────────────

    @Test
    fun `the destination is the session on screen`() {
        val on = session(3)
        val r = Handoff.Voice.route("l16", on, listOf(on))
        assertEquals(3, r.target?.session)
        assertEquals("l16", r.target?.machine)
        assertNull(r.why)
        assertTrue(r.label.isNotBlank(), "the user must be told what they are talking to")
    }

    @Test
    fun `a session that has gone refuses before the recogniser opens`() {
        // The point of resolving first: a microphone that opens for a session
        // that is not there wastes the sentence somebody just said.
        val r = Handoff.Voice.route("l16", session(3), live = emptyList())
        assertNull(r.target)
        assertEquals(Reply.Refusal.GONE.message, r.why)
    }

    @Test
    fun `every refusal comes from Reply check and not from a second vocabulary`() {
        // Two lists of reasons a message cannot be delivered is how one of them
        // ends up saying something the other does not. Each case below is
        // asserted to carry Reply's OWN sentence.
        val on = session(3, started = 100L)
        assertEquals(
            Reply.Refusal.RECYCLED.message,
            Handoff.Voice.route("l16", on, listOf(session(3, started = 999L))).why,
        )
        assertEquals(
            Reply.Refusal.EXITED.message,
            Handoff.Voice.route("l16", on, listOf(session(3, started = 100L, state = AgentStates.EXITED))).why,
        )
        assertEquals(
            Reply.Refusal.PAUSED.message,
            Handoff.Voice.route(
                "l16",
                on,
                listOf(session(3, started = 100L).copy(paused = true)),
            ).why,
        )
        // OTHER_MACHINE is NOT reachable here and that is correct rather than a
        // gap: route() builds the target from the machine it is handed, so the
        // two can never disagree. Reply.Refusal.OTHER_MACHINE guards a target
        // that was STORED — one composed against a notification and sent after
        // the phone connected somewhere else — which is a different moment.
    }

    @Test
    fun `the route carries started, because a session id is not an identity`() {
        // The daemon reuses a session number after a prune, so a route that
        // carried only the number could deliver a dictated sentence into a
        // stranger's terminal. Asserted on the value AND through the guard that
        // reads it, because a field nothing compares is a field that can be
        // wrong for ever.
        val on = session(3, started = 1_726_000_000L)
        val r = Handoff.Voice.route("l16", on, listOf(on))
        assertEquals(1_726_000_000L, r.target?.started)
        assertEquals(
            Reply.Refusal.RECYCLED,
            Reply.check(r.target!!, "l16", listOf(session(3, started = 9L))),
            "a route whose started no longer matches was not refused by Reply.check",
        )
    }

    @Test
    fun `push-to-talk has exactly one refusal of its own, and it is about this phone`() {
        // Everything else is Reply's. If this list grows, the two vocabularies
        // have started to diverge.
        assertEquals(
            listOf(Handoff.Voice.Refusal.NO_RECOGNISER),
            Handoff.Voice.Refusal.entries.toList(),
        )
        assertTrue(Handoff.Voice.Refusal.NO_RECOGNISER.message.isNotBlank())
    }

    // ── the microphone is never open without somewhere to go ────────────────

    @Test
    fun `a refused route leaves the microphone shut`() {
        val refused = Handoff.Voice.route("l16", session(3), live = emptyList())
        val s = Handoff.Voice.start(Handoff.Voice.State(), refused, now = 10L)
        assertEquals(Handoff.Voice.Phase.ERROR, s.phase)
        assertNull(s.target)
        assertFalse(Handoff.Voice.micOpen(s), "the microphone opened with nowhere to send")
        assertTrue(s.error.isNotBlank(), "a refusal that says nothing is 'it did nothing'")
    }

    @Test
    fun `the target is held for the whole time the microphone is open`() {
        val route = Handoff.Voice.route("l16", session(3), listOf(session(3)))
        var s = Handoff.Voice.start(Handoff.Voice.State(), route, now = 0L)
        assertEquals(Handoff.Voice.Phase.RECORDING, s.phase)
        assertNotNull(s.target)
        s = Handoff.Voice.stop(s)
        assertNotNull(s.target, "the target was dropped on the way to transcribing")
        s = Handoff.Voice.transcribed(s, "rebase onto main")
        assertEquals(Handoff.Voice.Phase.DELIVERING, s.phase)
        assertEquals(3, s.target?.session, "the target moved between recording and delivery")
    }

    @Test
    fun `the frozen target does not follow the phone to another session`() {
        // What "frozen" buys. The list moves while somebody is talking; the
        // destination does not.
        val route = Handoff.Voice.route("l16", session(3), listOf(session(3)))
        val recording = Handoff.Voice.start(Handoff.Voice.State(), route, now = 0L)
        val later = Handoff.Voice.route("l16", session(8), listOf(session(8)))
        assertEquals(8, later.target?.session)
        assertEquals(
            3,
            Handoff.Voice.transcribed(Handoff.Voice.stop(recording), "go on").target?.session,
            "the delivery followed the list instead of the frozen target",
        )
    }

    @Test
    fun `transcribing and delivering cannot be interrupted by another press`() {
        val route = Handoff.Voice.route("l16", session(3), listOf(session(3)))
        val transcribing = Handoff.Voice.stop(Handoff.Voice.start(Handoff.Voice.State(), route, 0L))
        assertEquals(
            transcribing,
            Handoff.Voice.start(transcribing, route, now = 50L),
            "a second press started a recording on top of words already in flight",
        )
        val delivering = Handoff.Voice.transcribed(transcribing, "hello")
        assertEquals(delivering, Handoff.Voice.start(delivering, route, now = 60L))
    }

    @Test
    fun `an open microphone expires at ninety seconds`() {
        assertEquals(
            90_000L,
            Handoff.Voice.MAX_MS,
            "the cap must agree with apex-shell's pushtotalk.js MAX_MS",
        )
        val route = Handoff.Voice.route("l16", session(3), listOf(session(3)))
        val s = Handoff.Voice.start(Handoff.Voice.State(), route, now = 1_000L)
        assertFalse(Handoff.Voice.expired(s, now = 1_000L + 89_999L))
        assertTrue(Handoff.Voice.expired(s, now = 1_000L + 90_000L))
        assertEquals(90_000L, Handoff.Voice.remainingMs(s, now = 1_000L))
        assertEquals(0L, Handoff.Voice.remainingMs(s, now = 1_000L + 200_000L), "the countdown went negative")
        assertEquals(
            0L,
            Handoff.Voice.remainingMs(Handoff.Voice.State(), now = 5L),
            "an idle microphone has no time left to show",
        )
    }

    @Test
    fun `an empty transcript is not delivered`() {
        // A room too loud to hear in. Sending the empty string would put
        // nothing in the agent's input line while the phone reported it had
        // spoken, and nobody would learn why the agent never answered.
        val route = Handoff.Voice.route("l16", session(3), listOf(session(3)))
        val s = Handoff.Voice.stop(Handoff.Voice.start(Handoff.Voice.State(), route, 0L))
        val blank = Handoff.Voice.transcribed(s, "   \n ")
        assertEquals(Handoff.Voice.Phase.ERROR, blank.phase)
        assertTrue(blank.error.isNotBlank())
    }

    @Test
    fun `a delivered transcript returns to idle with nothing held`() {
        val route = Handoff.Voice.route("l16", session(3), listOf(session(3)))
        var s = Handoff.Voice.transcribed(
            Handoff.Voice.stop(Handoff.Voice.start(Handoff.Voice.State(), route, 0L)),
            "ship it",
        )
        s = Handoff.Voice.delivered(s)
        assertEquals(Handoff.Voice.Phase.IDLE, s.phase)
        assertNull(s.target, "a delivered target stayed behind for the next recording")
        assertEquals("", s.text, "a delivered transcript stayed behind")
    }

    @Test
    fun `a failure from any phase shuts the microphone`() {
        // A guard that only fired in one phase would leave the microphone open
        // in a state nothing could leave, which is the failure the cap exists
        // for happening on purpose.
        val route = Handoff.Voice.route("l16", session(3), listOf(session(3)))
        for (s in listOf(
            Handoff.Voice.State(),
            Handoff.Voice.start(Handoff.Voice.State(), route, 0L),
            Handoff.Voice.stop(Handoff.Voice.start(Handoff.Voice.State(), route, 0L)),
        )) {
            val failed = Handoff.Voice.failed(s, "the recogniser stopped")
            assertEquals(Handoff.Voice.Phase.ERROR, failed.phase)
            assertFalse(Handoff.Voice.micOpen(failed))
            assertNull(failed.target)
        }
        assertTrue(
            Handoff.Voice.failed(Handoff.Voice.State(), "  ").error.isNotBlank(),
            "a failure with no reason must still say something",
        )
    }

    // ── the transcript comes back to another activity's screen ──────────────

    @Test
    fun `a transcript only lands in the box it was dictated for`() {
        // The frozen target, surviving the move of the review step onto the
        // phone. The recogniser is ANOTHER ACTIVITY: this one is stopped while
        // it runs, and what comes back arrives at whatever the user has since
        // navigated to. Without this, one agent's dictated answer lands in
        // another agent's box, where the person reads it as their own words and
        // presses Send.
        val on = session(3)
        val route = Handoff.Voice.route("l16", on, listOf(on))
        val delivering = Handoff.Voice.transcribed(
            Handoff.Voice.stop(Handoff.Voice.start(Handoff.Voice.State(), route, 0L)),
            "run the tests",
        )
        assertTrue(Handoff.Voice.landsOn(delivering, route.target))
        assertFalse(
            Handoff.Voice.landsOn(delivering, Reply.Target("l16", 8, 1_000L)),
            "a transcript landed in a different session's box",
        )
        assertFalse(
            Handoff.Voice.landsOn(delivering, Reply.Target("katana", 3, 1_000L)),
            "a transcript crossed to another machine",
        )
        assertFalse(
            Handoff.Voice.landsOn(delivering, null),
            "a transcript landed with nothing on screen",
        )
        assertTrue(Handoff.Voice.LANDED_ELSEWHERE.isNotBlank())
    }

    @Test
    fun `a recycled id is not the same box`() {
        // Compares all three values, not the number. A transcript returning to
        // the same id on a DIFFERENT session must be refused too.
        val on = session(3, started = 100L)
        val route = Handoff.Voice.route("l16", on, listOf(on))
        val delivering = Handoff.Voice.transcribed(
            Handoff.Voice.stop(Handoff.Voice.start(Handoff.Voice.State(), route, 0L)),
            "carry on",
        )
        assertFalse(
            Handoff.Voice.landsOn(delivering, Reply.Target("l16", 3, 999L)),
            "the id was reused and the transcript landed in the new session's box",
        )
    }

    @Test
    fun `nothing lands before there is a transcript`() {
        val on = session(3)
        val route = Handoff.Voice.route("l16", on, listOf(on))
        assertFalse(Handoff.Voice.landsOn(Handoff.Voice.State(), route.target))
    }

    // ── what a transcript becomes on the wire ───────────────────────────────

    @Test
    fun `a transcript reaches the reply box unsubmitted, and Send is what submits`() {
        // The rule of this feature, placed where the reviewer is. A recogniser's
        // guess is read by a person before it becomes an instruction; on a
        // phone that person is here, so the guess goes into an editable box and
        // the existing Send button is the only thing that terminates it.
        val forReview = Handoff.Voice.forReview("  delete the old branches  ")
        assertFalse(forReview.contains('\r'), "a transcript submitted itself")
        assertFalse(forReview.contains('\n'), "a transcript submitted itself")
        assertEquals("delete the old branches ", forReview)
        // And the one path to the wire still terminates, because a person read
        // it and pressed Send.
        assertTrue(
            Reply.bytes(forReview).endsWith("\r"),
            "the single send path stopped submitting, so a reply now waits unseen",
        )
        assertEquals(
            """{"cmd":"input","id":3,"data":"delete the old branches\r"}""",
            Agentd.input(3, Reply.bytes(forReview)),
        )
    }

    // ── clipboard ───────────────────────────────────────────────────────────

    @Test
    fun `a one-line clipboard is plain`() {
        val s = Handoff.Clipboard.inspect("cargo test -p apex-agentd")
        assertEquals(1, s.lines)
        assertFalse(s.submits)
        assertTrue(s.controlBytes.isEmpty())
        assertTrue(s.isPlain)
    }

    @Test
    fun `an interior newline submits, and that is what the user is told`() {
        // Input writes raw bytes to a PTY and the daemon appends nothing, so
        // the newline in the middle of a copied stack trace IS the return key.
        val s = Handoff.Clipboard.inspect("first line\nsecond line")
        assertTrue(s.submits, "a paste that presses return was reported as plain")
        assertEquals(2, s.lines)
        assertFalse(s.isPlain)
    }

    @Test
    fun `a trailing newline does not count as a submit or as an extra line`() {
        // Copied text almost always ends in one. Treating it as a hazard would
        // put a warning in front of every ordinary paste, and a warning that
        // fires every time is a warning nobody reads.
        val s = Handoff.Clipboard.inspect("one line\n")
        assertFalse(s.submits, "a trailing newline was reported as an interior one")
        assertEquals(1, s.lines)
        assertTrue(s.isPlain)
    }

    @Test
    fun `CRLF is one press of return and not two`() {
        val s = Handoff.Clipboard.inspect("a\r\nb\r\nc")
        assertEquals(3, s.lines, "CRLF was counted as two line breaks")
        assertTrue(s.submits)
    }

    @Test
    fun `an escape byte is reported, because a TUI acts on it`() {
        val s = Handoff.Clipboard.inspect("plain[2Jmore")
        assertEquals(listOf(0x1b), s.controlBytes)
        assertFalse(s.isPlain)
        assertFalse(s.submits, "there is no line break here; only a control byte")
    }

    @Test
    fun `tab is not reported as a hazard`() {
        // Indented code is the commonest thing anybody pastes at an agent. A
        // tab is displayed, not acted on.
        val s = Handoff.Clipboard.inspect("\tindented()")
        assertTrue(s.controlBytes.isEmpty(), "a tab was reported as a control byte")
        assertTrue(s.isPlain)
    }

    @Test
    fun `a clipboard over the cap is reported rather than silently clipped`() {
        val s = Handoff.Clipboard.inspect("x".repeat(Handoff.Clipboard.MAX_CHARS + 1))
        assertTrue(s.truncated)
        assertFalse(s.isPlain)
        assertFalse(Handoff.Clipboard.inspect("x".repeat(Handoff.Clipboard.MAX_CHARS)).truncated)
    }

    @Test
    fun `the safe choice sends one line and the deliberate one sends all of it`() {
        val text = "git log --oneline\nrm -rf build\n"
        assertEquals(
            "git log --oneline",
            Handoff.Clipboard.forReview(text, Handoff.Clipboard.Choice.FIRST_LINE),
            "FIRST_LINE must not carry the second command with it",
        )
        assertEquals(text, Handoff.Clipboard.forReview(text, Handoff.Clipboard.Choice.EVERYTHING))
    }

    @Test
    fun `neither clipboard choice terminates by itself`() {
        // What lands in the box is what the person reads. A choice that
        // terminated would show them a paste and send a submitted one.
        for (c in Handoff.Clipboard.Choice.entries) {
            assertFalse(
                Handoff.Clipboard.forReview("some text", c).endsWith("\r"),
                "$c appended a carriage return",
            )
        }
    }

    @Test
    fun `a clipboard send is clipped to the cap it reported`() {
        val big = "y".repeat(Handoff.Clipboard.MAX_CHARS * 2)
        assertEquals(
            Handoff.Clipboard.MAX_CHARS,
            Handoff.Clipboard.forReview(big, Handoff.Clipboard.Choice.EVERYTHING).length,
        )
    }

    @Test
    fun `the send-path rule catches a paste the app never handled`() {
        // The placement that matters, and the reason it is not on the clipboard
        // button. A user long-presses the reply field and taps the keyboard's
        // own Paste; no code of this app's runs. Reply.bytes then trims the
        // ENDS only, so the interior newline survives — and on a PTY that
        // newline IS the return key, submitting line one and typing lines two
        // and three into whatever the agent asks next.
        val pasted = "Traceback (most recent call last):\n  File \"x.py\", line 3\nValueError"
        assertTrue(
            Handoff.Clipboard.inspect(pasted).submits,
            "a paste that presses return partway through was not caught at the send",
        )
        // And the proof that Reply.bytes does not save you: the hazard is still
        // in what it produces.
        assertTrue(
            Reply.bytes(pasted).count { it == '\n' } > 0,
            "Reply.bytes was assumed to strip interior newlines, and it does not",
        )
        // An ordinary typed sentence passes, so the gate is not a wall.
        assertTrue(Handoff.Clipboard.inspect("rerun the failing test").isPlain)
    }

    // ── the file criterion, and the permissions that guard it ───────────────

    @Test
    fun `the file handoff refusal names what is missing rather than apologising`() {
        assertTrue(Handoff.Files.WHY.isNotBlank())
        assertTrue(
            Handoff.Files.WHY.contains("apex agent send"),
            "the refusal must name the thing that DOES work, or it is an apology",
        )
    }

    @Test
    fun `every storage permission the picker would make unnecessary is named`() {
        // Asserted here as well as in the manifest test so the list itself is
        // one thing rather than two copies: the manifest test reads this list.
        assertTrue(Handoff.Files.FORBIDDEN_PERMISSIONS.contains("android.permission.READ_MEDIA_IMAGES"))
        assertTrue(Handoff.Files.FORBIDDEN_PERMISSIONS.contains("android.permission.READ_EXTERNAL_STORAGE"))
        assertTrue(Handoff.Files.FORBIDDEN_PERMISSIONS.contains("android.permission.MANAGE_EXTERNAL_STORAGE"))
        assertTrue(
            Handoff.Files.FORBIDDEN_PERMISSIONS.all { it.startsWith("android.permission.") },
            "a name that is not a permission cannot be checked against a manifest",
        )
    }
}
