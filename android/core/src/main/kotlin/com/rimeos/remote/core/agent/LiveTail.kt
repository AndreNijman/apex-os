package com.rimeos.remote.core.agent

import com.rimeos.remote.core.term.Terminal

/**
 * A `peek` reply: the tail of a session's PTY output and the size it was
 * drawn at.
 *
 * [data] is raw terminal bytes, not text. It is the END of the daemon's
 * scrollback ring, so it can begin in the middle of an escape sequence or of
 * a UTF-8 character, and it only means anything when replayed into a
 * terminal of [cols] by [rows] — cursor movements are absolute, and a TUI
 * that drew its status line at row 40 of a 120-column screen draws garbage
 * into an 80 by 24 one.
 */
class Peek(
    val id: Int,
    val data: ByteArray,
    val cols: Int,
    val rows: Int,
    /** The session's state when the tail was read, as `list` would say it. */
    val state: String,
    /**
     * How many bytes were asked for.
     *
     * The reply does not say whether the ring held more than it sent, and the
     * difference decides whether the first byte is the start of the output or
     * the middle of something: a tail as long as the request was almost
     * certainly cut, and one shorter than it is everything there was.
     */
    val requested: Int = Agentd.PEEK_MAX,
) {
    /** Whether the machine had more than it sent, so the first byte may be mid-sequence. */
    val truncated: Boolean get() = data.size >= minOf(requested, Agentd.PEEK_MAX)

    override fun toString(): String = "Peek(id=$id, ${data.size} bytes, ${cols}x$rows, $state)"
}

/**
 * The bottom of a session's screen, without attaching to it.
 *
 * ## Why it goes through the terminal emulator
 *
 * Because the bytes are a program's drawing instructions, not its output. An
 * agent's UI redraws its spinner in place with a carriage return, moves the
 * cursor up to repaint a box, clears to the end of the line — so "the last
 * few lines of the bytes" is mostly escape sequences and half-overwritten
 * text, and a phone that showed it would show noise. Replaying the tail into
 * the same [Terminal] the terminal screen uses, at the size the daemon says
 * the PTY is, gives the text a person would see on the desktop's bottom rows.
 *
 * It is a FRESH terminal every time, and that is correct rather than
 * wasteful: the tail is a window onto a stream this phone did not see the
 * start of, so the state an earlier peek left behind (a colour, a scroll
 * region, the alternate screen) is not evidence about this one.
 *
 * ## What it deliberately does not do
 *
 * It never answers the program. A tail that contains `ESC[6n` makes the
 * emulator queue a cursor report, and on an attached terminal that report is
 * written back to the PTY. Here it is dropped — nothing reads
 * [Terminal.takeResponses] — because a preview that typed into the agent's
 * input would be the one thing worse than no preview.
 */
object LiveTail {
    /** How many rows the session screen shows. */
    const val LINES: Int = 15

    /** Enough history above the grid to fill [LINES] after a clear-to-bottom. */
    private const val SCROLLBACK: Int = 200

    /** Bounds on a size the daemon reports, so a hostile number cannot allocate a screen. */
    private const val MAX_COLS: Int = 500
    private const val MAX_ROWS: Int = 300

    /**
     * How far into a truncated tail to look for a place to start cleanly.
     *
     * A tail that begins mid-escape would otherwise print the escape's
     * parameters as text — `5;1H` on the first line. Resyncing at the first
     * ESC or line break inside this window costs at most a partial first line
     * of a preview, which is the line least likely to matter.
     */
    private const val RESYNC_WINDOW: Int = 128

    /**
     * The bottom [max] lines of the screen the tail draws, blank ends trimmed.
     *
     * Trailing blank rows are dropped — below the cursor a TUI's screen is
     * empty, and fifteen rows of nothing is not a preview — and so are
     * leading ones, which on a short tail are only the part of the grid
     * nothing has drawn on yet. Blank rows BETWEEN text are kept, because
     * they are layout the program chose.
     */
    fun lines(peek: Peek, max: Int = LINES): List<String> {
        if (max <= 0) return emptyList()
        val terminal = replay(peek)
        return terminal.read { screen ->
            val all = ArrayList<String>(screen.totalLines)
            // The alternate screen has no history by design — a full-screen
            // program owns the grid — so this is the grid there and history
            // plus grid on the normal screen, which is what the emulator's own
            // `totalLines` already means.
            for (i in 0 until screen.totalLines) {
                all.add(screen.lineAt(i)?.text()?.trimEnd().orEmpty())
            }
            while (all.isNotEmpty() && all.last().isBlank()) all.removeAt(all.size - 1)
            val kept = all.takeLast(max)
            kept.dropWhile { it.isBlank() }
        }
    }

    /**
     * The last line with something to read on it, for an Agent Center row.
     *
     * "Something to read" excludes a line drawn only in box-drawing and block
     * characters: an agent's input box ends in `╰────╯`, and a row that
     * previewed the bottom border of a box would be a row that says nothing.
     */
    fun lastLine(peek: Peek): String? =
        lines(peek, max = SCROLLBACK).lastOrNull { hasText(it) }?.trim()

    private fun hasText(line: String): Boolean =
        line.codePoints().anyMatch { cp ->
            !Character.isWhitespace(cp) && cp !in 0x2500..0x259F
        }

    private fun replay(peek: Peek): Terminal {
        val cols = peek.cols.coerceIn(1, MAX_COLS)
        val rows = peek.rows.coerceIn(1, MAX_ROWS)
        val terminal = Terminal(cols, rows, scrollbackLimit = SCROLLBACK)
        val bytes = if (peek.truncated) resync(peek.data) else peek.data
        terminal.feed(bytes)
        // Taken and thrown away: see the class note. Nothing here may answer
        // the program.
        terminal.takeResponses()
        return terminal
    }

    /**
     * Skip a partial UTF-8 character and, when one is near, a partial escape.
     *
     * Only for a [Peek.truncated] tail: one shorter than the request is the
     * whole output, and cutting its first line would lose the one line that
     * is certainly real.
     *
     * Continuation bytes (`10xxxxxx`) at the very start are always a torn
     * character and always skipped. Beyond that the tail is cut at the first
     * ESC, CR or LF within [RESYNC_WINDOW] bytes — but only when the data does
     * not already start with one, and only when one is found: a tail of plain
     * text with no control bytes in it is kept whole.
     */
    internal fun resync(data: ByteArray): ByteArray {
        var start = 0
        while (start < data.size && (data[start].toInt() and 0xC0) == 0x80) start++
        if (start < data.size && !isBoundary(data[start])) {
            val limit = minOf(data.size, start + RESYNC_WINDOW)
            for (i in start until limit) {
                if (isBoundary(data[i])) {
                    start = i
                    break
                }
            }
        }
        return if (start == 0) data else data.copyOfRange(start, data.size)
    }

    private fun isBoundary(b: Byte): Boolean = b == ESC || b == CR || b == LF

    private const val ESC: Byte = 0x1b
    private const val CR: Byte = '\r'.code.toByte()
    private const val LF: Byte = '\n'.code.toByte()
}
