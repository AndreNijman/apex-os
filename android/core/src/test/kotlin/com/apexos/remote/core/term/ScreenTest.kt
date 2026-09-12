package com.apexos.remote.core.term

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertNotEquals
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

/**
 * P1-055: "scrollback, search, selection, copy/paste."
 *
 * Search and selection are the two that are easy to write and easy to get
 * subtly wrong, because both have to work in **grid columns** while the text a
 * reader sees is a string of characters — and the two stop agreeing the moment
 * a wide character or an emoji appears on the line.
 */
class ScreenTest {
    private val esc = Ansi.ESC

    private fun loaded(): Terminal {
        val t = Terminal(24, 4)
        t.feed("first line here\r\n")
        t.feed("second LINE here\r\n")
        t.feed("third line\r\n")
        t.feed("fourth\r\n")
        t.feed("fifth line\r\n")
        t.feed("sixth")
        return t
    }

    // ---- search ----------------------------------------------------------

    @Test
    fun `search reaches into the scrollback and not only the visible rows`() {
        val t = loaded()
        assertTrue(t.screen.scrollbackSize > 0, "the fixture did not scroll")
        val hits = t.screen.search("first")
        assertEquals(1, hits.size, "got $hits")
        assertEquals(0, hits[0].line, "the match is on the first line ever printed, which has scrolled off")
        assertTrue(hits[0].line < t.screen.scrollbackSize, "the match is in the scrollback")
    }

    @Test
    fun `search is case-insensitive by default and exact when asked`() {
        val t = loaded()
        // first / second (LINE) / third / fifth. `fourth` and `sixth` do not.
        assertEquals(4, t.screen.search("line").size, "got ${t.screen.search("line")}")
        assertEquals(3, t.screen.search("line", ignoreCase = false).size, "`LINE` is not `line`")
    }

    @Test
    fun `search finds several matches on one line, in order`() {
        val t = Terminal(40, 2)
        t.feed("ab ab ab")
        val hits = t.screen.search("ab")
        assertEquals(listOf(0, 3, 6), hits.map { it.col }, "got $hits")
        assertEquals(listOf(0, 0, 0), hits.map { it.line })
    }

    @Test
    fun `search reports a grid column and not a character index`() {
        // The two differ the moment a double-width character is on the line.
        // A highlight drawn at the character index would sit two columns to
        // the left of the match, for every match after the first CJK glyph.
        val t = Terminal(40, 2)
        t.feed("日本語 target")
        val hits = t.screen.search("target")
        assertEquals(1, hits.size, "got $hits")
        // Three wide characters, three columns of tail, one space: column 7.
        assertEquals(7, hits[0].col, "the column must count cells, not characters")
        assertEquals("target", t.screen.textBetween(Pos(0, 7), Pos(0, 12)))
    }

    @Test
    fun `search ignores the escape sequences that coloured the text`() {
        // A word split by an SGR escape is one word to the reader, and the
        // escape left no cell behind, so it must be one word here.
        val t = Terminal(40, 2)
        t.feed("err${esc}[31mor happened")
        assertEquals(1, t.screen.search("error").size, "the colour change split the word")
    }

    @Test
    fun `search that finds nothing finds nothing, and an empty needle matches nowhere`() {
        val t = loaded()
        assertEquals(emptyList<Match>(), t.screen.search("nonexistent"))
        assertEquals(emptyList<Match>(), t.screen.search(""), "an empty search must not match every position")
    }

    // ---- selection -------------------------------------------------------

    @Test
    fun `a selection drops the blanks a terminal padded the line with`() {
        val t = Terminal(20, 2)
        t.feed("short")
        // The line is twenty cells wide; only five were typed.
        assertEquals("short", t.screen.textBetween(Pos(0, 0), Pos(0, 19)))
    }

    @Test
    fun `a selection that stops mid-line keeps its interior spacing`() {
        val t = Terminal(20, 2)
        t.feed("a    b")
        assertEquals("a    b", t.screen.textBetween(Pos(0, 0), Pos(0, 5)))
        assertEquals("    ", t.screen.textBetween(Pos(0, 1), Pos(0, 4)))
    }

    @Test
    fun `a selection across a wrapped line does not insert a newline`() {
        // The failure this prevents is a copied command that will not run:
        // one long line that came back with a newline every eighty columns.
        val t = Terminal(10, 3)
        t.feed("git commit --amend --no-edit")
        val whole = t.screen.textBetween(Pos(0, 0), Pos(2, 7))
        assertEquals("git commit --amend --no-edit", whole)
        assertTrue(!whole.contains('\n'), "a wrapped line was broken: $whole")
    }

    @Test
    fun `a selection across real lines keeps the newlines`() {
        val t = Terminal(20, 3)
        t.feed("one\r\ntwo\r\nthree")
        assertEquals("one\ntwo\nthree", t.screen.textBetween(Pos(0, 0), Pos(2, 4)))
    }

    @Test
    fun `a selection given backwards means the same as one given forwards`() {
        val t = Terminal(20, 2)
        t.feed("hello world")
        val forwards = t.screen.textBetween(Pos(0, 0), Pos(0, 4))
        val backwards = t.screen.textBetween(Pos(0, 4), Pos(0, 0))
        assertEquals(forwards, backwards)
        assertEquals("hello", forwards)
    }

    @Test
    fun `a selection reaching into the scrollback picks up the history`() {
        val t = loaded()
        val text = t.screen.textBetween(Pos(0, 0), Pos(1, 15))
        assertEquals("first line here\nsecond LINE here", text)
    }

    @Test
    fun `a selection spanning a wide character copies it once`() {
        val t = Terminal(20, 2)
        t.feed("[日本]")
        assertEquals("[日本]", t.screen.textBetween(Pos(0, 0), Pos(0, 5)))
        // And a selection that stops on the tail of a wide character still
        // includes the character it belongs to rather than half of it.
        assertEquals("[日", t.screen.textBetween(Pos(0, 0), Pos(0, 2)))
    }

    // ---- scrollback ------------------------------------------------------

    @Test
    fun `the transcript is the history and the screen, in order`() {
        val t = loaded()
        assertEquals(
            "first line here\nsecond LINE here\nthird line\nfourth\nfifth line\nsixth",
            t.screen.transcript(),
        )
    }

    @Test
    fun `dropping lines off the front is counted so a caller can fix its indices`() {
        val t = Terminal(8, 2, scrollbackLimit = 2)
        repeat(6) { t.feed("l$it\r\n") }
        assertEquals(2, t.screen.scrollbackSize)
        assertNotEquals(0L, t.screen.dropped, "a ring that drops silently moves every saved index")
        val before = t.screen.dropped
        t.feed("more\r\n")
        assertTrue(t.screen.dropped > before, "the count must keep moving")
    }
}
