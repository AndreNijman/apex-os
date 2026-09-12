package com.apexos.remote.core.term

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertNotEquals
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

/**
 * P1-055: "supports scrollback, search, text selection, copy/paste …".
 *
 * Four of that criterion's six words are decisions rather than drawings, and
 * this is where they are held to account. The composable that draws a
 * [Snapshot] cannot be tested on a machine with no device; every rule it obeys
 * can be.
 */
class ViewportTest {
    /** A terminal with [lines] numbered lines of history behind an 8-row grid. */
    private fun scrolled(lines: Int, cols: Int = 20, rows: Int = 8): Terminal {
        val t = Terminal(cols, rows, scrollbackLimit = 500)
        for (i in 0 until lines) t.feed("line $i\r\n")
        return t
    }

    private fun viewport(t: Terminal, height: Int = 8) = Viewport(t, height)

    @Test
    fun `a fresh viewport follows the output`() {
        val t = scrolled(40)
        val v = viewport(t)
        assertTrue(v.following)
        val snap = v.snapshot()
        assertEquals(8, snap.rows.size)
        // The last written line is on screen. `line 39` then an empty row,
        // because the final CRLF opened one.
        assertTrue(snap.text().contains("line 39"), "the newest output is not visible: ${snap.text()}")
    }

    @Test
    fun `scrolling back stops the view following, and scrolling to the end resumes it`() {
        val t = scrolled(40)
        val v = viewport(t)
        val bottom = v.snapshot().top

        v.scrollBy(-10)
        assertFalse(v.following, "scrolling back left the view following")
        assertEquals(bottom - 10, v.snapshot().top)

        v.scrollBy(10)
        assertTrue(v.following, "scrolling to the end did not resume following")
        assertEquals(bottom, v.snapshot().top)
    }

    @Test
    fun `a scrolled-back view holds still while output arrives above the fold`() {
        // The property the whole id scheme exists for. A viewport that stored
        // an absolute index would be correct here and wrong after an eviction;
        // the next test is the one that separates them.
        val t = scrolled(40)
        val v = viewport(t)
        v.scrollBy(-15)
        val before = v.snapshot()
        val text = before.text()

        for (i in 0 until 5) t.feed("new $i\r\n")

        val after = v.snapshot()
        assertEquals(text, after.text(), "the view moved when output arrived")
        assertFalse(after.following)
    }

    @Test
    fun `the view holds its content when the scrollback overflows and every index shifts`() {
        // A short scrollback, so eviction really happens. This is the
        // assertion a viewport holding absolute indices fails: `dropped` moves
        // and the same index names a different line.
        val t = Terminal(20, 4, scrollbackLimit = 20)
        for (i in 0 until 24) t.feed("line $i\r\n")
        val v = viewport(t, height = 4)
        v.scrollBy(-6)
        val held = v.snapshot().text()
        val droppedBefore = t.read { it.dropped }

        // Enough output to push lines off the front of the history.
        for (i in 0 until 12) t.feed("more $i\r\n")
        val droppedAfter = t.read { it.dropped }
        assertTrue(
            droppedAfter > droppedBefore,
            "the scrollback did not overflow, so this test proved nothing " +
                "(dropped $droppedBefore -> $droppedAfter)",
        )

        assertEquals(held, v.snapshot().text(), "the view drifted when lines were evicted")
    }

    @Test
    fun `the cursor is not drawn while looking at history`() {
        val t = scrolled(40)
        val v = viewport(t)
        assertTrue(v.snapshot().cursorRow >= 0, "the cursor should be on screen while following")
        v.scrollBy(-20)
        val snap = v.snapshot()
        assertEquals(-1, snap.cursorRow, "a cursor was placed in a view that is showing history")
        assertFalse(snap.cursorVisible)
    }

    // ---- search ---------------------------------------------------------

    @Test
    fun `search finds every occurrence and starts at the last one`() {
        val t = Terminal(30, 4, scrollbackLimit = 200)
        t.feed("error one\r\nfine\r\nerror two\r\nfine\r\nerror three\r\n")
        val v = viewport(t, height = 4)
        assertEquals(3, v.find("error"))
        assertEquals(2, v.current, "a terminal search should start at the newest match, not the oldest")
    }

    @Test
    fun `next and previous wrap in both directions`() {
        val t = Terminal(30, 4, scrollbackLimit = 200)
        t.feed("hit a\r\nhit b\r\nhit c\r\n")
        val v = viewport(t, height = 4)
        v.find("hit")
        assertEquals(2, v.current)
        assertTrue(v.findNext())
        assertEquals(0, v.current, "next from the last match should wrap to the first")
        assertTrue(v.findPrevious())
        assertEquals(2, v.current, "previous from the first should wrap to the last")
    }

    @Test
    fun `stepping to a match that is off screen scrolls it into view`() {
        val t = Terminal(30, 4, scrollbackLimit = 400)
        t.feed("needle at the top\r\n")
        for (i in 0 until 60) t.feed("filler $i\r\n")
        val v = viewport(t, height = 4)

        assertEquals(1, v.find("needle"))
        val snap = v.snapshot()
        assertTrue(
            snap.text().contains("needle"),
            "the match was not scrolled into view: showing lines ${snap.top}..${snap.top + snap.rows.size}",
        )
        assertFalse(v.following, "scrolling to an old match left the view following the output")
    }

    @Test
    fun `every match is highlighted and exactly one of them is the current one`() {
        val t = Terminal(30, 6, scrollbackLimit = 200)
        t.feed("aa bb aa\r\ncc aa dd\r\n")
        val v = viewport(t, height = 6)
        assertEquals(3, v.find("aa"))
        val spans = v.snapshot().rows.flatMap { it.spans }
        assertEquals(3, spans.size, "not every match was highlighted")
        assertEquals(
            1,
            spans.count { it.kind == SpanKind.CURRENT_MATCH },
            "there should be exactly one current match",
        )
        assertEquals(2, spans.count { it.kind == SpanKind.MATCH })
    }

    @Test
    fun `an empty query clears the matches rather than matching everything`() {
        val t = Terminal(20, 4)
        t.feed("something\r\n")
        val v = viewport(t, height = 4)
        v.find("some")
        assertEquals(1, v.matchCount)
        assertEquals(0, v.find(""))
        assertEquals(-1, v.current)
        assertTrue(v.snapshot().rows.all { it.spans.isEmpty() })
    }

    @Test
    fun `search is case-insensitive by default and can be told not to be`() {
        val t = Terminal(30, 4, scrollbackLimit = 100)
        t.feed("Error here\r\nerror there\r\n")
        val v = viewport(t, height = 4)
        assertEquals(2, v.find("error"))
        assertEquals(1, v.find("error", ignoreCase = false))
    }

    // ---- selection ------------------------------------------------------

    @Test
    fun `a selection copies what is between its ends, without the trailing blanks`() {
        val t = Terminal(40, 4)
        t.feed("hello world\r\n")
        val v = viewport(t, height = 4)
        v.selectFrom(Pos(0, 0))
        v.selectTo(Pos(0, 39))
        assertEquals(
            "hello world",
            v.selectedText(),
            "a full-width selection copied the cells the terminal pads a line with",
        )
    }

    @Test
    fun `a selection that stops mid-line keeps its interior spacing`() {
        val t = Terminal(40, 4)
        t.feed("a  b  c\r\n")
        val v = viewport(t, height = 4)
        v.selectFrom(Pos(0, 0))
        v.selectTo(Pos(0, 6))
        assertEquals("a  b  c", v.selectedText())
    }

    @Test
    fun `a selection across a wrapped line comes back as one line, so the command runs`() {
        // The rule `Screen.textBetween` documents, exercised through the
        // viewport: a long command that wrapped is one command.
        val t = Terminal(10, 4)
        t.feed("echo abcdefghijkl")
        val v = viewport(t, height = 4)
        v.selectFrom(Pos(0, 0))
        v.selectTo(Pos(1, 9))
        val text = v.selectedText()
        assertEquals("echo abcdefghijkl", text)
        assertFalse(text!!.contains('\n'), "a wrapped line was copied with a newline in the middle")
    }

    @Test
    fun `a selection dragged backwards copies the same text as one dragged forwards`() {
        val t = Terminal(40, 4)
        t.feed("first\r\nsecond\r\n")
        val v = viewport(t, height = 4)
        v.selectFrom(Pos(0, 0))
        v.selectTo(Pos(1, 5))
        val forwards = v.selectedText()
        v.clearSelection()
        v.selectFrom(Pos(1, 5))
        v.selectTo(Pos(0, 0))
        assertEquals(forwards, v.selectedText())
    }

    @Test
    fun `a word is a run of non-blanks, so a path is one word`() {
        val t = Terminal(60, 4)
        t.feed("run --worktree=/var/tmp/x now\r\n")
        val v = viewport(t, height = 4)
        assertTrue(v.selectWord(Pos(0, 10)))
        assertEquals("--worktree=/var/tmp/x", v.selectedText())
    }

    @Test
    fun `selecting a blank selects nothing rather than selecting the gap`() {
        val t = Terminal(20, 4)
        t.feed("a   b\r\n")
        val v = viewport(t, height = 4)
        assertFalse(v.selectWord(Pos(0, 2)))
        assertFalse(v.hasSelection)
        assertNull(v.selectedText())
    }

    @Test
    fun `a selection survives the eviction its line indices do not`() {
        val t = Terminal(20, 4, scrollbackLimit = 12)
        t.feed("MARKER here\r\n")
        for (i in 0 until 8) t.feed("pad $i\r\n")
        val v = viewport(t, height = 4)
        assertTrue(v.selectWord(Pos(0, 0)))
        assertEquals("MARKER", v.selectedText())

        // Push the marker line further back, but not off the end.
        for (i in 0 until 4) t.feed("more $i\r\n")
        assertEquals("MARKER", v.selectedText(), "the selection followed an index instead of a line")
    }

    @Test
    fun `selection spans land on every row the selection crosses`() {
        val t = Terminal(20, 6)
        t.feed("aaaa\r\nbbbb\r\ncccc\r\n")
        val v = viewport(t, height = 6)
        v.selectFrom(Pos(0, 2))
        v.selectTo(Pos(2, 1))
        val snap = v.snapshot()
        val rows = snap.rows.filter { row -> row.spans.any { it.kind == SpanKind.SELECTION } }
        assertEquals(3, rows.size, "a three-row selection did not mark three rows")
        assertEquals(2, rows[0].spans.first { it.kind == SpanKind.SELECTION }.from)
        assertEquals(19, rows[0].spans.first { it.kind == SpanKind.SELECTION }.to)
        assertEquals(0, rows[1].spans.first { it.kind == SpanKind.SELECTION }.from)
        assertEquals(1, rows[2].spans.first { it.kind == SpanKind.SELECTION }.to)
    }

    // ---- what the renderer is handed ------------------------------------

    @Test
    fun `a snapshot is a copy, so output arriving mid-draw cannot tear a row`() {
        val t = Terminal(20, 4)
        t.feed("original\r\n")
        val v = viewport(t, height = 4)
        val snap = v.snapshot()
        val before = snap.rows[0].text()
        // The pump thread's job, done while a renderer would be walking rows.
        t.feed("[H" + "overwritten")
        assertEquals(before, snap.rows[0].text(), "the snapshot shared arrays with the live screen")
        assertNotEquals(before, v.snapshot().rows[0].text())
    }

    @Test
    fun `a tap maps back to the absolute position the selection API takes`() {
        val t = scrolled(40)
        val v = viewport(t)
        v.scrollBy(-12)
        val snap = v.snapshot()
        val pos = snap.posAt(2, 3)!!
        assertEquals(snap.top + 2, pos.line)
        assertEquals(3, pos.col)
        // And it round-trips: selecting the word at that position finds text
        // from the row the snapshot actually showed.
        assertTrue(v.selectWord(pos))
        assertTrue(snap.rows[2].text().contains(v.selectedText()!!))
    }

    @Test
    fun `the height the renderer reports is what decides how many rows come back`() {
        val t = scrolled(60)
        val v = viewport(t, height = 8)
        assertEquals(8, v.snapshot().rows.size)
        // A rotation into landscape: fewer rows, more columns. The terminal
        // resize is `PtyAttachment.resize`'s job; the viewport's is to stop
        // handing back rows that no longer fit.
        v.height = 4
        assertEquals(4, v.snapshot().rows.size)
        v.height = 20
        assertEquals(20, v.snapshot().rows.size)
    }

    @Test
    fun `a resize keeps the history the view is looking at`() {
        val t = scrolled(60, cols = 20, rows = 8)
        val v = viewport(t, height = 8)
        v.scrollBy(-20)
        val before = v.snapshot().rows.map { it.text().trimEnd() }

        // Portrait to landscape.
        t.resize(40, 6)
        v.height = 6
        val after = v.snapshot()
        assertEquals(40, after.cols, "the grid did not take the new width")
        assertEquals(6, after.rows.size)
        assertTrue(
            after.rows.map { it.text().trimEnd() }.first() in before,
            "rotating lost the place in the history: was ${before.first()}, now ${after.rows.first().text().trimEnd()}",
        )
    }
}
