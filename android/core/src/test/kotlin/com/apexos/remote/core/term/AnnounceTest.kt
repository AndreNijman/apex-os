package com.apexos.remote.core.term

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertNotNull
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

/**
 * What a screen reader is given, and what it is interrupted with.
 *
 * Driven through a real [Terminal] and a real [Viewport] rather than through
 * hand-built snapshots: the thing under test is a decision about what a
 * *terminal* is doing, and a fixture that constructs its own [Snapshot] would
 * be asserting against this test's idea of a terminal instead of the one the
 * app runs.
 */
class AnnounceTest {

    private val terminal = Terminal(cols = 20, rows = 6)
    private val viewport = Viewport(terminal, height = 6)

    private fun frame(): Snapshot = viewport.snapshot()

    // ---- what is there to read ------------------------------------------

    @Test
    fun `the visible buffer is the node's text, one line per row`() {
        terminal.feed("first\r\nsecond\r\nthird")
        val reading = TerminalReading.of(frame())
        assertEquals(listOf("first", "second", "third"), reading.text.lines())
    }

    @Test
    fun `the padding a grid puts on every row is not read out`() {
        // Every line in a terminal is `cols` cells wide whether or not anything
        // was written to them, so the naive reading of a six-character line on
        // an eighty-column screen is six characters and seventy-four spaces.
        terminal.feed("hi")
        val reading = TerminalReading.of(frame())
        assertEquals("hi", reading.text)
    }

    @Test
    fun `the state names where the view is, how much there is, and the cursor`() {
        terminal.feed("one\r\ntwo\r\n")
        val state = TerminalReading.of(frame()).state
        assertTrue(state.startsWith("Live."), state)
        assertTrue(state.contains("of 6"), state)
        // Line and column are the ones a person would say out loud: the third
        // line and the first column, not index 2 and index 0.
        assertTrue(state.contains("Cursor on line 3, column 1."), state)
    }

    @Test
    fun `a scrolled view says so before anything else`() {
        repeat(20) { terminal.feed("line $it\r\n") }
        viewport.scrollBy(-5)
        val state = TerminalReading.of(frame()).state
        assertTrue(state.startsWith("Scrolled back."), state)
        // Because the whole point of saying it is that output is still
        // arriving somewhere the user is not being shown.
        assertFalse(state.contains("Live"), state)
    }

    @Test
    fun `a terminal with nothing on it still has something to say`() {
        // It is a clickable node — a tap raises the keyboard — and a clickable
        // node with no words is announced as "button" and nothing else.
        val reading = TerminalReading.of(frame())
        assertTrue(reading.isEmpty)
        assertTrue(reading.text.isNotBlank(), "an empty terminal must still be describable")
    }

    @Test
    fun `the cursor line is offered separately, and is null when it is off screen`() {
        terminal.feed("alpha\r\nbeta")
        assertEquals("beta", TerminalReading.of(frame()).cursorLine)
        repeat(20) { terminal.feed("line $it\r\n") }
        viewport.scrollBy(-10)
        assertNull(TerminalReading.of(frame()).cursorLine)
    }

    // ---- what is said out loud -------------------------------------------

    private val announcer = TerminalAnnouncer()

    /** Seed the announcer the way the first frame of a real attach does. */
    private fun seeded(): TerminalAnnouncer {
        assertNull(announcer.onFrame(frame()), "the first frame must never announce")
        return announcer
    }

    @Test
    fun `the replay an attach starts with is not read aloud`() {
        // `apex-agentd` sends up to 256 KiB of history to every attaching
        // client, and the watchdog attaches by itself. Announcing that history
        // would read a stale transcript out loud on every reconnect.
        terminal.feed("something that was already on the machine\r\n")
        assertNull(announcer.onFrame(frame()))
    }

    @Test
    fun `a finished line is announced once`() {
        seeded()
        terminal.feed("build ok\r\n")
        assertEquals("build ok", announcer.onFrame(frame()))
        // The same frame again, and a frame after an unrelated repaint, say
        // nothing: a line is news once.
        assertNull(announcer.onFrame(frame()))
    }

    @Test
    fun `the line being written is not announced until the cursor leaves it`() {
        seeded()
        // A prompt, a progress bar, a half-typed command. Reading it out mid
        // write announces something that was never on screen.
        terminal.feed("Downloading 43")
        assertNull(announcer.onFrame(frame()))
        terminal.feed("%\r\n")
        assertEquals("Downloading 43%", announcer.onFrame(frame()))
    }

    @Test
    fun `nothing is announced while somebody is reading the history`() {
        // Seeded on an empty terminal, the way a fresh attach seeds, and the
        // output arrives while the view is held at the top. Everything on
        // screen is then both OLD and unannounced, which is the one shape
        // where "have these lines been read out" and "should they be" differ —
        // an announcer without the following check reads the history somebody
        // is in the middle of exploring, out loud, over the top of them.
        seeded()
        repeat(20) { terminal.feed("old $it\r\n") }
        viewport.toTop()
        assertNull(announcer.onFrame(frame()), "a reader who has scrolled must not be talked over")

        // And it is not silenced for ever by having scrolled: coming back to
        // the live output hears what arrived.
        viewport.toBottom()
        assertNotNull(announcer.onFrame(frame()))
    }

    @Test
    fun `a burst is one utterance, and it says how much it left out`() {
        seeded()
        // Twenty lines in one frame is an ordinary `make`. Twenty
        // announcements would mean hearing the first two words of each.
        // Twenty lines, of which only the last six are still on a six-row
        // screen: the rest went into the scrollback between one frame and the
        // next and were never displayed. Saying how many is the honest form of
        // a summary — reading out lines the user could not have seen is not.
        repeat(20) { terminal.feed("compiling file-$it.c\r\n") }
        val said = announcer.onFrame(frame())
        assertNotNull(said, "twenty new lines must produce an announcement")
        said!!
        assertTrue(said.lines().size <= 9, "one utterance, not twenty: $said")
        assertTrue(said.startsWith("15 earlier lines."), said)
        assertTrue(said.endsWith("compiling file-19.c"), said)
    }

    @Test
    fun `a line that repeats itself is announced once`() {
        seeded()
        // A spinner that paints `Working…` on a new line every second is one
        // announcement, not sixty.
        terminal.feed("Working\r\n")
        assertEquals("Working", announcer.onFrame(frame()))
        terminal.feed("Working\r\n")
        assertNull(announcer.onFrame(frame()))
    }

    @Test
    fun `a reconnect is silent, and then the new session is heard`() {
        repeat(10) { terminal.feed("before $it\r\n") }
        seeded()
        terminal.feed("last thing before the drop\r\n")
        assertNotNull(announcer.onFrame(frame()))


        // What `PtyAttachment` does on every attempt: reset, then feed the
        // daemon's replay. Line numbers restart, so an announcer keyed on them
        // would either read the replay aloud or go silent for as long as the
        // old session had been running.
        terminal.reset()
        repeat(3) { terminal.feed("replayed $it\r\n") }
        assertNull(announcer.onFrame(frame()), "the replay is not news")

        terminal.feed("genuinely new\r\n")
        assertEquals("genuinely new", announcer.onFrame(frame()))
    }

    @Test
    fun `a full-screen TUI repainting its grid is announced once and then stays quiet`() {
        // The limit, asserted rather than left to be discovered. On the
        // alternate screen a program owns the whole grid and rewrites it in
        // place: the same line numbers hold different text every frame. This
        // announcer keys on line numbers, so it reads a TUI's first paint and
        // then nothing — which is the right way round. The alternative, a
        // reader that announced every repaint of Claude Code's interface,
        // would make the app unusable with TalkBack on. The buffer is still
        // there to be read; it is simply not shouted.
        terminal.feed("[?1049h")
        seeded()
        terminal.feed("[H" + "menu\r\nitem one\r\nitem two\r\n")
        assertNotNull(announcer.onFrame(frame()))
        terminal.feed("[H" + "menu\r\nitem one\r\nitem TWO\r\n")
        assertNull(announcer.onFrame(frame()))
    }

    @Test
    fun `blank lines are read but not announced`() {
        seeded()
        terminal.feed("\r\n\r\n\r\n")
        assertNull(announcer.onFrame(frame()), "three empty lines are not worth an interruption")
        // They are still part of the screen: a terminal's blank lines are
        // layout, and a reading that closed the gaps would describe a screen
        // nobody is looking at.
        terminal.feed("after\r\n")
        assertTrue(TerminalReading.of(frame()).text.contains("\n\n"))
    }

    @Test
    fun `reset forgets everything, because the terminal it was tracking is gone`() {
        seeded()
        terminal.feed("a line\r\n")
        assertEquals("a line", announcer.onFrame(frame()))
        announcer.reset()
        // Seeded again by the next frame, so the same words can be news again.
        assertNull(announcer.onFrame(frame()))
        terminal.feed("a line\r\n")
        assertEquals("a line", announcer.onFrame(frame()))
    }
}
