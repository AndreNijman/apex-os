package com.apexos.remote.core.term

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertNotEquals
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

/**
 * The low-bandwidth criterion, stated as a property rather than as a feeling.
 *
 * ## Why this is the honest form of "low-bandwidth behaviour is tested"
 *
 * A test that throttled a socket and then looked at the screen would prove
 * nothing: it would pass on a parser that buffered until it saw a complete
 * escape sequence and hung the first time one never arrived. What a slow link
 * actually does to a terminal is **split the byte stream in places nobody
 * chose** — in the middle of `ESC[38;2;215;119;87m`, between the two bytes of
 * `é`, between `ESC` and `]`. So the property is:
 *
 * > feeding a stream in any number of pieces produces exactly the screen that
 * > feeding it in one piece produces, and exactly the same answers back.
 *
 * Three runs per fixture. **Byte at a time** is the strongest of them: it
 * splits at every boundary at once, so a parser holding any state in a local
 * rather than in a field diverges on the first escape sequence. The two-piece
 * splits then cover the other shape — a large `feed` resuming mid-sequence
 * after another large one — at up to [SPLIT_SAMPLES] boundaries per fixture,
 * evenly spaced, which for everything but the largest recording is every
 * boundary there is.
 *
 * The answers are compared too, and that is not decoration: `ESC[6n` split
 * between the `6` and the `n` must still produce exactly one cursor report,
 * and a parser that restarted its CSI state would produce none or two.
 */
class TerminalSplitTest {
    private val esc = Ansi.ESC

    /**
     * A digest of everything a screen can be, as one number.
     *
     * A string would be more readable and is what the failure message builds;
     * this is what the loop compares, because a fixture with a hundred lines
     * of scrollback times twelve thousand split positions is a great many
     * strings to allocate to say "no change".
     */
    private fun digest(t: Terminal): Long {
        var h = -0x340d631b7bdddcdbL // FNV-1a 64-bit offset basis
        fun mix(v: Long) {
            h = h xor v
            h *= 0x100000001b3L
        }
        mix(t.cols.toLong()); mix(t.rows.toLong())
        mix(t.cursorRow.toLong()); mix(t.cursorCol.toLong())
        mix(if (t.onAlternate) 1 else 2)
        mix(if (t.applicationCursorKeys) 3 else 4)
        mix(if (t.autoWrap) 5 else 6)
        mix(if (t.bracketedPaste) 7 else 8)
        mix(if (t.insertMode) 9 else 10)
        mix(if (t.mouseReporting) 11 else 12)
        mix(if (t.cursorVisible) 13 else 14)
        mix(t.fg.toLong()); mix(t.bg.toLong()); mix(t.attrs.toLong())
        for (ch in t.title) mix(ch.code.toLong())
        val s = t.screen
        mix(s.totalLines.toLong())
        for (i in 0 until s.totalLines) {
            val line = s.lineAt(i) ?: continue
            // Every cell WITH its colours and attributes. A digest of the text
            // alone would let a split corrupt a colour and still pass.
            for (c in 0 until line.cols) {
                mix(line.code[c].toLong())
                mix(line.fg[c].toLong())
                mix(line.bg[c].toLong())
                mix(line.attrs[c].toLong())
            }
            mix(if (line.wrapped) 1L else 0L)
        }
        return h
    }

    /** The readable form, built only when something has already gone wrong. */
    private fun describe(t: Terminal): String = buildString {
        append("cursor=").append(t.cursorRow).append(',').append(t.cursorCol)
        append(" alt=").append(t.onAlternate)
        append(" ckm=").append(t.applicationCursorKeys)
        append(" pen=").append(Colour.describe(t.fg)).append('/').append(Attrs.describe(t.attrs))
        append(" title=").append(t.title).append('\n')
        append(t.screen.transcript())
    }

    private class Run(val digest: Long, val answers: String, val text: String)

    private fun whole(bytes: ByteArray): Run {
        val t = Terminal(80, 24)
        t.feed(bytes)
        return Run(digest(t), t.takeResponses().toString(Charsets.ISO_8859_1), describe(t))
    }

    private fun twoPieces(bytes: ByteArray, at: Int): Run {
        val t = Terminal(80, 24)
        t.feed(bytes, 0, at)
        val first = t.takeResponses()
        t.feed(bytes, at, bytes.size - at)
        val second = t.takeResponses()
        // The answers are concatenated across the two calls: a caller writes
        // them to the PTY as they appear, so the sequence is what matters and
        // not which call produced which part of it.
        return Run(digest(t), (first + second).toString(Charsets.ISO_8859_1), describe(t))
    }

    private fun oneByteAtATime(bytes: ByteArray): Run {
        val t = Terminal(80, 24)
        val answers = StringBuilder()
        val one = ByteArray(1)
        for (b in bytes) {
            one[0] = b
            t.feed(one)
            answers.append(t.takeResponses().toString(Charsets.ISO_8859_1))
        }
        return Run(digest(t), answers.toString(), describe(t))
    }

    private fun assertSplitInvariant(name: String, bytes: ByteArray): Int {
        val reference = whole(bytes)
        val single = oneByteAtATime(bytes)
        assertEquals(
            reference.digest, single.digest,
            "$name: one byte at a time produced a different screen\n--- whole ---\n${reference.text}" +
                "\n--- byte at a time ---\n${single.text}",
        )
        assertEquals(reference.answers, single.answers, "$name: one byte at a time produced different answers")

        var checked = 0
        val step = maxOf(1, (bytes.size + 1) / SPLIT_SAMPLES)
        var at = 0
        while (at <= bytes.size) {
            val split = twoPieces(bytes, at)
            assertEquals(
                reference.digest, split.digest,
                "$name: a split after byte $at changed the screen\n--- whole ---\n${reference.text}" +
                    "\n--- split ---\n${split.text}",
            )
            assertEquals(reference.answers, split.answers, "$name: a split after byte $at changed the answers")
            checked++
            at += step
        }
        return checked
    }

    @Test
    fun `a hand-built stream with every sequence shape survives every split`() {
        val stream = buildString {
            append("plain text ")
            append("$esc[1;31mbold red$esc[0m ")
            append("$esc[38;2;215;119;87mtruecolor$esc[39m ")
            append("$esc[38:5:46mindexed$esc[m ")
            append("é→😀日本 ")
            append("$esc]0;a title\u0007")
            append("$esc[2;5H")
            append("$esc[?1049h$esc[2J$esc[?1h$esc[?2004h")
            append("alt screen\r\n")
            append("$esc[6n$esc[c$esc[>0q$esc[?2026\$p")
            append("${esc}P+q544e$esc\\")
            append("$esc]11;?$esc\\")
            append("$esc[?1049l")
            append("back$esc[K\r\n")
            repeat(30) { append("line $it\r\n") }
        }.toByteArray(Charsets.UTF_8)
        val checked = assertSplitInvariant("hand-built", stream)
        assertTrue(checked > 200, "only $checked split positions were tried")
    }

    @Test
    fun `every recorded TUI stream survives every split`() {
        var fixtures = 0
        for (name in TuiFixtures.ALL) {
            assertSplitInvariant(name, TuiFixtures.read(name))
            fixtures++
        }
        assertEquals(TuiFixtures.ALL.size, fixtures)
        assertTrue(fixtures >= 6, "only $fixtures fixtures exist; the test resources are missing")
    }

    @Test
    fun `the digest is capable of telling two screens apart`() {
        // The anti-vacuity guard. Every assertion above compares digests, so a
        // digest that collapsed everything to one value would make all of them
        // pass on any parser at all — including one that printed nothing.
        assertNotEquals(whole("$esc[31mred".toByteArray()).digest, whole("$esc[32mred".toByteArray()).digest, "colour")
        assertNotEquals(whole("abc".toByteArray()).digest, whole("abd".toByteArray()).digest, "text")
        assertNotEquals(whole("a".toByteArray()).digest, whole("a$esc[1m".toByteArray()).digest, "pen")
        assertNotEquals(whole("a".toByteArray()).digest, whole("$esc[?1ha".toByteArray()).digest, "mode")
        assertNotEquals(whole("a".toByteArray()).digest, whole("a$esc[H".toByteArray()).digest, "cursor")
        assertNotEquals(
            whole("x".toByteArray()).digest,
            whole("$esc]0;t\u0007x".toByteArray()).digest,
            "title",
        )
    }

    private companion object {
        /**
         * How many two-piece split positions to try per fixture.
         *
         * Every boundary, for anything up to this size — which is all but the
         * largest recording. Beyond it the positions are evenly spaced, and the
         * byte-at-a-time run has already split at every boundary anyway.
         */
        const val SPLIT_SAMPLES = 4096
    }
}
