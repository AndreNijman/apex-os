package com.apexos.remote.core.term

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

/**
 * P1-055: "works with Claude/OpenCode/Codex/Gemini TUIs without rewriting
 * them."
 *
 * ## What that criterion actually means, measured rather than assumed
 *
 * It sounds like a rendering claim and it is not. Every one of these agents
 * opens by **interrogating the terminal** and waiting for the answers. Recorded
 * against a real PTY on 2026-09-12:
 *
 * | agent    | unanswered | answered |
 * |----------|-----------:|---------:|
 * | opencode |  284 bytes | 12665 bytes |
 * | codex    | 2660 bytes |  3128 bytes |
 * | claude   | 2192 bytes |  2220 bytes |
 *
 * opencode is the stark case: 284 bytes of questions, then silence, then
 * nothing on screen at all. So "without rewriting them" is, in practice, "the
 * terminal answers what they ask". These tests hold the emulator to the
 * questions the recordings show them asking.
 *
 * ## What is NOT claimed
 *
 * **gemini is not installed on this machine**, so it has no fixture and nothing
 * here was run against it. Three of the four are covered.
 *
 * Nor is this a claim about pixels. It is a claim about a grid: the text these
 * programs painted lands where they put it, in the colours they asked for.
 */
class TuiCompatibilityTest {
    private val esc = Ansi.ESC

    private fun feed(name: String, cols: Int = 80, rows: Int = 24): Terminal {
        val t = Terminal(cols, rows)
        t.feed(TuiFixtures.read(name))
        return t
    }

    // ---- the questions ---------------------------------------------------

    @Test
    fun `every agent's opening questions are answered`() {
        // One assertion per agent, naming the agent, so a failure says which
        // TUI would hang rather than "some fixture".
        for ((agent, pair) in TuiFixtures.AGENTS) {
            val t = Terminal(80, 24)
            t.feed(TuiFixtures.read(pair.first))
            val answers = t.takeResponses().toString(Charsets.ISO_8859_1)
            assertTrue(
                answers.isNotEmpty(),
                "$agent asked the terminal questions and got nothing back; it would wait forever",
            )
        }
    }

    @Test
    fun `a cursor position report comes back for every ESC-bracket-6n`() {
        // codex and opencode both open with `ESC[6n` and block on the reply.
        for (name in listOf(TuiFixtures.CODEX_QUERIES, TuiFixtures.OPENCODE_QUERIES)) {
            val raw = TuiFixtures.read(name).toString(Charsets.ISO_8859_1)
            val asked = Regex(Regex.escape("$esc[6n")).findAll(raw).count()
            assertTrue(asked > 0, "$name does not contain a DSR; the fixture is wrong")
            val t = Terminal(80, 24)
            t.feed(TuiFixtures.read(name))
            val answers = t.takeResponses().toString(Charsets.ISO_8859_1)
            val answered = Regex("${Regex.escape(esc)}\\[\\d+;\\d+R").findAll(answers).count()
            assertEquals(asked, answered, "$name asked $asked times and was answered $answered times")
        }
    }

    @Test
    fun `a device attributes report comes back for every ESC-bracket-c`() {
        for (name in TuiFixtures.ALL) {
            val raw = TuiFixtures.read(name).toString(Charsets.ISO_8859_1)
            // `ESC[c` exactly — not `ESC[>c`, which is the secondary report.
            val asked = Regex("${Regex.escape(esc)}\\[c").findAll(raw).count()
            if (asked == 0) continue
            val t = Terminal(80, 24)
            t.feed(TuiFixtures.read(name))
            val answers = t.takeResponses().toString(Charsets.ISO_8859_1)
            val answered = Regex("${Regex.escape(esc)}\\[\\?62;22c").findAll(answers).count()
            assertEquals(asked, answered, "$name: DA1 asked $asked, answered $answered")
        }
    }

    @Test
    fun `the foreground and background colour queries are answered with the theme`() {
        val t = Terminal(80, 24)
        t.defaultForeground = Colour.rgb(0xCD, 0xD6, 0xF4)
        t.defaultBackground = Colour.rgb(0x1A, 0x28, 0x2A)
        t.feed(TuiFixtures.read(TuiFixtures.OPENCODE_QUERIES))
        val answers = t.takeResponses().toString(Charsets.ISO_8859_1)
        assertTrue(answers.contains("]10;rgb:cdcd/d6d6/f4f4"), "OSC 10 unanswered: $answers")
        assertTrue(answers.contains("]11;rgb:1a1a/2828/2a2a"), "OSC 11 unanswered: $answers")
    }

    @Test
    fun `a mode query answers set or reset for modes we have and zero for modes we do not`() {
        val t = Terminal(80, 24)
        t.feed("$esc[?2004h$esc[?2004\$p$esc[?7\$p$esc[?1016\$p")
        val answers = t.takeResponses().toString(Charsets.ISO_8859_1)
        assertTrue(answers.contains("$esc[?2004;1\$y"), "bracketed paste is on and must report 1: $answers")
        assertTrue(answers.contains("$esc[?7;1\$y"), "autowrap is on: $answers")
        // 1016 is SGR-pixel mouse reporting, which this build does not have.
        // Answering "reset" would tell the TUI the feature exists and is off,
        // and it would then turn it on and rely on it.
        assertTrue(answers.contains("$esc[?1016;0\$y"), "an unknown mode must report 0, not 2: $answers")
    }

    @Test
    fun `a terminfo capability query is refused rather than ignored`() {
        // opencode sends `DCS + q 4d73 ST`. `DCS 0 + r <name> ST` means "I do
        // not have that capability" and is a complete answer; silence is what
        // makes it wait.
        val t = Terminal(80, 24)
        t.feed("${esc}P+q4d73$esc\\")
        val answers = t.takeResponses().toString(Charsets.ISO_8859_1)
        assertEquals("${esc}P0+r4d73$esc\\", answers)
    }

    @Test
    fun `the window size reports agree with the grid`() {
        val t = Terminal(100, 30)
        t.feed("$esc[18t$esc[14t")
        val answers = t.takeResponses().toString(Charsets.ISO_8859_1)
        assertTrue(answers.contains("$esc[8;30;100t"), "the character size report: $answers")
        // The pixel report must be consistent with the character one, because
        // an image-capable TUI divides one by the other to get a cell.
        val pixels = Regex("${Regex.escape(esc)}\\[4;(\\d+);(\\d+)t").find(answers)
            ?: error("no pixel size report in $answers")
        val height = pixels.groupValues[1].toInt()
        val width = pixels.groupValues[2].toInt()
        assertEquals(0, height % 30, "the pixel height must divide by the rows")
        assertEquals(0, width % 100, "the pixel width must divide by the columns")
    }

    @Test
    fun `nothing a TUI asks leaks onto the screen`() {
        // The failure this guards is the loudest one there is: an OSC or a DCS
        // whose body is printed as text, which fills the screen with escape
        // bodies and makes the terminal look broken.
        for (name in TuiFixtures.ALL) {
            val t = feed(name)
            val text = t.screen.transcript()
            for (marker in listOf("+q", "rgb:", "1337;", "i=31337", "Capabilities", "\$p", "\$y")) {
                assertFalse(text.contains(marker), "$name leaked '$marker' onto the screen")
            }
        }
    }

    // ---- what they painted ----------------------------------------------

    @Test
    fun `Claude Code's welcome lands on the grid`() {
        val t = feed(TuiFixtures.CLAUDE_SESSION)
        val text = t.screen.transcript()
        assertTrue(text.contains("Welcome to Claude Code"), "got:\n$text")
        assertTrue(text.contains("Choose the text style"), "got:\n$text")
        // Claude Code paints with truecolor. Find its brand colour on the grid
        // rather than trusting that the sequence was parsed.
        assertTrue(
            anyCell(t) { it.fg == Colour.rgb(215, 119, 87) },
            "the truecolor foreground `ESC[38;2;215;119;87m` reached no cell",
        )
    }

    @Test
    fun `Codex's sign-in screen lands on the grid, in its 256-colour palette`() {
        val t = feed(TuiFixtures.CODEX_SESSION)
        val text = t.screen.transcript()
        assertTrue(text.contains("Welcome to Codex"), "got:\n$text")
        assertTrue(text.contains("Sign in with ChatGPT"), "got:\n$text")
        assertTrue(
            anyCell(t) { it.fg == Colour.indexed(6) },
            "codex's `ESC[38;5;6;49m` reached no cell",
        )
        assertTrue(anyCell(t) { Attrs.has(it.attrs, Attrs.DIM) }, "codex's SGR 2 reached no cell")
    }

    @Test
    fun `opencode paints its full-screen interface once its questions are answered`() {
        val t = feed(TuiFixtures.OPENCODE_SESSION)
        // It switched to the alternate screen, which is what a full-screen
        // application does and what the unanswered recording never reaches.
        assertTrue(t.onAlternate, "opencode never got as far as the alternate screen")
        assertTrue(
            anyCell(t) { it.fg == Colour.rgb(255, 255, 255) },
            "opencode's truecolor fill reached no cell",
        )
        assertTrue(t.mouseReporting, "opencode asks for mouse reporting and the mode was not tracked")
    }

    @Test
    fun `the unanswered recordings really do paint less than the answered ones`() {
        // The anti-vacuity guard for this whole file. If the two recordings
        // were identical, every claim above about answering would be empty.
        for ((agent, pair) in TuiFixtures.AGENTS) {
            val quiet = TuiFixtures.read(pair.first)
            val answered = TuiFixtures.read(pair.second)
            assertTrue(
                answered.size > quiet.size,
                "$agent: the answered recording (${answered.size}B) is not larger than the " +
                    "unanswered one (${quiet.size}B), so the fixtures prove nothing",
            )
        }
        // And the extreme case, named. Unanswered, opencode gets as far as
        // switching to the alternate screen and then paints **nothing on it**:
        // it is still waiting for the cursor report it asked for in its ninth
        // byte. Answered, the same program fills the grid.
        val silent = feed(TuiFixtures.OPENCODE_QUERIES)
        assertTrue(silent.onAlternate, "the fixture no longer reaches the alternate screen")
        assertEquals("", silent.screen.transcript().trim(), "the unanswered opencode run put text on screen")
        val loud = feed(TuiFixtures.OPENCODE_SESSION)
        assertTrue(
            loud.screen.transcript().trim().isNotEmpty(),
            "the answered opencode run painted nothing either, so answering changed nothing",
        )
    }

    private fun anyCell(t: Terminal, predicate: (Cell) -> Boolean): Boolean {
        val s = t.screen
        for (i in 0 until s.totalLines) {
            val line = s.lineAt(i) ?: continue
            for (c in 0 until line.cols) {
                if (predicate(line.cellAt(c))) return true
            }
        }
        return false
    }
}
