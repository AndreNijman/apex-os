package com.apexos.remote.core.term

/**
 * What a person is looking at, and what they have picked out of it.
 *
 * ## Why this is not in the Compose layer
 *
 * Everything P1-055 asks for after "streams real PTY output" is a decision
 * rather than a drawing: where the view sits in the history, which occurrence
 * of a word is the current one, which cells are selected, what text that
 * selection actually copies. None of those can be tested on a machine with no
 * device, and all of them are where the defects are. So they live here, as
 * pure functions over a [Terminal], and the composable only draws a
 * [Snapshot].
 *
 * ## Line ids, because absolute indices move
 *
 * [Screen]'s own documentation is explicit: lines evicted off the front of the
 * scrollback shift every absolute index, and it publishes [Screen.dropped] so
 * a caller can notice. A viewport that remembered "I am looking at line 300"
 * would therefore drift upward through the output at exactly the rate the
 * scrollback overflows — slowly, invisibly, and only on a session long enough
 * that nobody would connect the two.
 *
 * So every position this class stores is an **id**: `dropped + index`, which
 * counts lines that have ever existed and is therefore monotonic. Converting
 * is [idOf] and [indexOf], and a position whose line has been evicted converts
 * to something before the start, which the callers clamp.
 *
 * ## Following, and what stops it
 *
 * A terminal at the bottom scrolls with its output; one scrolled back holds
 * still while output arrives above the fold. That is [following], and it is a
 * *mode*, not a computed comparison — a viewport that decided it by checking
 * whether the offset happened to be zero would start following again by
 * accident every time the user scrolled to the very bottom and then a line
 * arrived. [toBottom] is the only thing that resumes it, and the screen's
 * "jump to latest" button is the only thing that calls it.
 */
class Viewport(
    val terminal: Terminal,
    /** How many rows the screen can show. Set by the renderer on every layout. */
    var height: Int = 24,
) {
    /**
     * The id of the top visible line, when not [following].
     *
     * Meaningless while following, and deliberately not kept up to date then:
     * a field that is only correct in one mode is less trouble than one that
     * is correct in both and read in the wrong one.
     */
    private var topId: Long = 0

    /** Whether the view rides the bottom of the output. */
    var following: Boolean = true
        private set

    // ---- selection ------------------------------------------------------

    /** Where a drag started, in line ids. */
    private var anchor: IdPos? = null

    /** Where it is now. */
    private var caret: IdPos? = null

    /** Whether anything is selected. */
    val hasSelection: Boolean get() = anchor != null && caret != null

    // ---- search ---------------------------------------------------------

    var query: String = ""
        private set

    private var matches: List<IdMatch> = emptyList()

    /** Which match is the current one, or -1 when there are none. */
    var current: Int = -1
        private set

    val matchCount: Int get() = matches.size

    // ---- scrolling ------------------------------------------------------

    /**
     * Move by [lines]; negative is towards the past.
     *
     * Scrolling down past the last line resumes following rather than leaving
     * the view one line short of the bottom, because "scroll to the end" and
     * "follow the output" are the same intention expressed with a finger.
     */
    fun scrollBy(lines: Int) = terminal.read { screen ->
        if (lines == 0) return@read
        val top = if (following) followingTop(screen) else indexOf(topId, screen)
        moveTo(top + lines, screen)
    }

    /** Put the line with this **absolute index** at the top of the view. */
    fun scrollToIndex(index: Int) = terminal.read { screen -> moveTo(index, screen) }

    /** Show the line with this absolute index, scrolling as little as possible. */
    fun reveal(index: Int) = terminal.read { screen ->
        val top = if (following) followingTop(screen) else indexOf(topId, screen)
        when {
            index < top -> moveTo(index, screen)
            index >= top + height -> moveTo(index - height + 1, screen)
            else -> Unit
        }
    }

    /** Ride the output again. */
    fun toBottom() {
        following = true
    }

    /** The oldest line there is. */
    fun toTop() = terminal.read { screen -> moveTo(0, screen) }

    private fun moveTo(index: Int, screen: Screen) {
        val last = maxOf(0, screen.totalLines - height)
        val clamped = index.coerceIn(0, last)
        if (clamped >= last) {
            // At or past the bottom: follow, rather than sit one line short
            // and stop moving when the next line arrives.
            following = true
        } else {
            following = false
            topId = idOf(clamped, screen)
        }
    }

    private fun followingTop(screen: Screen): Int = maxOf(0, screen.totalLines - height)

    // ---- selection ------------------------------------------------------

    /** Start a selection at an absolute position. */
    fun selectFrom(pos: Pos) = terminal.read { screen ->
        anchor = idPos(pos, screen)
        caret = anchor
    }

    /** Extend it. */
    fun selectTo(pos: Pos) = terminal.read { screen ->
        if (anchor == null) anchor = idPos(pos, screen)
        caret = idPos(pos, screen)
    }

    fun clearSelection() {
        anchor = null
        caret = null
    }

    /**
     * The word under a position, as a selection.
     *
     * "Word" is the terminal definition rather than the prose one: a run of
     * anything that is not a space. A path, a flag, a git hash and a URL are
     * all one word, and they are what somebody double-taps a terminal to copy.
     * Splitting on punctuation would make `--worktree=/var/tmp/x` four
     * selections and none of them the useful one.
     */
    fun selectWord(pos: Pos): Boolean = terminal.read { screen ->
        val line = screen.lineAt(pos.line) ?: return@read false
        val col = pos.col.coerceIn(0, line.cols - 1)
        if (isBlank(line, col)) return@read false
        var from = col
        while (from > 0 && !isBlank(line, from - 1)) from--
        var to = col
        while (to < line.cols - 1 && !isBlank(line, to + 1)) to++
        anchor = idPos(Pos(pos.line, from), screen)
        caret = idPos(Pos(pos.line, to), screen)
        true
    }

    /** The whole line, edge to edge. Trailing blanks come off in [selectedText]. */
    fun selectLine(index: Int): Boolean = terminal.read { screen ->
        val line = screen.lineAt(index) ?: return@read false
        anchor = idPos(Pos(index, 0), screen)
        caret = idPos(Pos(index, line.cols - 1), screen)
        true
    }

    private fun isBlank(line: Line, col: Int): Boolean {
        val code = line.code[col]
        // A wide character's tail is part of the character before it, never a
        // gap: treating it as blank would end a selection in the middle of a
        // CJK glyph.
        if (code == Cell.WIDE_TAIL) return false
        return code == ' '.code || code == 0
    }

    /**
     * What the selection copies, or `null` when there is none.
     *
     * [Screen.textBetween] does the work and its two rules — trailing blanks
     * dropped, a wrapped line joined without a newline — are the reason a
     * copied command actually runs when it is pasted back.
     */
    fun selectedText(): String? = terminal.read { screen ->
        val a = anchor ?: return@read null
        val c = caret ?: return@read null
        val from = pos(a, screen)
        val to = pos(c, screen)
        val text = screen.textBetween(from, to)
        text.ifEmpty { null }
    }

    // ---- search ---------------------------------------------------------

    /**
     * Find [needle], and make the last occurrence the current one.
     *
     * The **last**, not the first, and this is the one search decision worth
     * arguing about. A terminal's interesting text is at the bottom: somebody
     * searching a build log for `error` wants the error that just happened,
     * not the one from the run before. Every editor starts at the top because
     * a document's interesting text is where the cursor is; a terminal's is
     * where the output is.
     */
    fun find(needle: String, ignoreCase: Boolean = true): Int = terminal.read { screen ->
        query = needle
        if (needle.isEmpty()) {
            matches = emptyList()
            current = -1
            return@read 0
        }
        matches = screen.search(needle, ignoreCase).map {
            IdMatch(idOf(it.line, screen), it.col, it.length)
        }
        current = matches.size - 1
        if (current >= 0) revealCurrent(screen)
        matches.size
    }

    /** The next occurrence, wrapping. */
    fun findNext(): Boolean = step(1)

    /** The previous one, wrapping. */
    fun findPrevious(): Boolean = step(-1)

    private fun step(by: Int): Boolean = terminal.read { screen ->
        if (matches.isEmpty()) return@read false
        current = ((current + by) % matches.size + matches.size) % matches.size
        revealCurrent(screen)
        true
    }

    fun clearSearch() {
        query = ""
        matches = emptyList()
        current = -1
    }

    private fun revealCurrent(screen: Screen) {
        val m = matches.getOrNull(current) ?: return
        val index = indexOf(m.id, screen)
        if (index < 0) return
        val top = if (following) followingTop(screen) else indexOf(topId, screen)
        when {
            index < top -> moveTo(index, screen)
            index >= top + height -> moveTo(index - height + 1, screen)
            else -> Unit
        }
    }

    // ---- what the renderer draws ----------------------------------------

    /**
     * Everything needed to paint one frame, copied out under the lock.
     *
     * Copied rather than handed out by reference, and the reason is not
     * caution: [Terminal.feed] runs on the pump thread and mutates the very
     * `IntArray`s a renderer would be walking. A drawing loop that read them
     * live would tear — half a row from before an escape sequence and half
     * from after — and on Android would do it inside a `Canvas` callback where
     * the fault surfaces as a rendering glitch nobody can reproduce.
     */
    fun snapshot(): Snapshot = terminal.read { screen ->
        val top = (if (following) followingTop(screen) else indexOf(topId, screen))
            .coerceIn(0, maxOf(0, screen.totalLines - 1))
        val count = minOf(height, screen.totalLines - top)
        val selection = selectionRange(screen)
        val currentMatch = matches.getOrNull(current)
        val rows = ArrayList<SnapshotRow>(maxOf(count, 0))
        for (i in 0 until count) {
            val index = top + i
            val line = screen.lineAt(index) ?: continue
            rows.add(
                SnapshotRow(
                    index = index,
                    code = line.code.copyOf(),
                    fg = line.fg.copyOf(),
                    bg = line.bg.copyOf(),
                    attrs = line.attrs.copyOf(),
                    wrapped = line.wrapped,
                    spans = spansFor(index, line.cols, selection, currentMatch, screen),
                ),
            )
        }
        // The cursor is a grid row; the view may be showing history, in which
        // case it is not on screen at all and `cursorRow` is deliberately -1
        // rather than clamped to an edge. A cursor drawn at the top of a
        // scrolled-back view is a cursor in the wrong place.
        val cursorIndex = screen.scrollbackSize + terminal.cursorRow
        val cursorRow = if (cursorIndex in top until top + count) cursorIndex - top else -1
        Snapshot(
            cols = screen.cols,
            rows = rows,
            top = top,
            totalLines = screen.totalLines,
            cursorRow = cursorRow,
            cursorCol = terminal.cursorCol,
            cursorVisible = terminal.cursorVisible && following,
            following = following,
            revision = terminal.revision,
            matchCount = matches.size,
            currentMatch = current,
        )
    }

    private fun selectionRange(screen: Screen): Pair<Pos, Pos>? {
        val a = anchor ?: return null
        val c = caret ?: return null
        val from = pos(a, screen)
        val to = pos(c, screen)
        return if (from <= to) from to to else to to from
    }

    private fun spansFor(
        index: Int,
        cols: Int,
        selection: Pair<Pos, Pos>?,
        currentMatch: IdMatch?,
        screen: Screen,
    ): List<Span> {
        val out = ArrayList<Span>(2)
        if (selection != null) {
            val (from, to) = selection
            if (index in from.line..to.line) {
                val first = if (index == from.line) from.col.coerceIn(0, cols - 1) else 0
                val last = if (index == to.line) to.col.coerceIn(0, cols - 1) else cols - 1
                if (first <= last) out.add(Span(first, last, SpanKind.SELECTION))
            }
        }
        // Every match on this line is highlighted; the current one is
        // highlighted differently. A search that marked only the current match
        // would make "47 matches" a number with nothing behind it.
        for (m in matches) {
            val at = indexOf(m.id, screen)
            if (at != index) continue
            val last = (m.col + m.length - 1).coerceAtMost(cols - 1)
            if (m.col > last) continue
            out.add(
                Span(
                    m.col,
                    last,
                    if (m === currentMatch) SpanKind.CURRENT_MATCH else SpanKind.MATCH,
                ),
            )
        }
        return out
    }

    // ---- ids ------------------------------------------------------------

    private fun idOf(index: Int, screen: Screen): Long = screen.dropped + index

    private fun indexOf(id: Long, screen: Screen): Int = (id - screen.dropped).toInt()

    private fun idPos(pos: Pos, screen: Screen): IdPos = IdPos(idOf(pos.line, screen), pos.col)

    private fun pos(p: IdPos, screen: Screen): Pos = Pos(indexOf(p.id, screen).coerceAtLeast(0), p.col)

    private data class IdPos(val id: Long, val col: Int)

    private data class IdMatch(val id: Long, val col: Int, val length: Int)
}

/** A run of columns the renderer paints differently. */
data class Span(val from: Int, val to: Int, val kind: SpanKind)

enum class SpanKind { SELECTION, MATCH, CURRENT_MATCH }

/**
 * One row of a [Snapshot].
 *
 * The four arrays are copies and the renderer owns them; [index] is the
 * absolute line index they came from, which is what a tap converts back into
 * a [Pos].
 */
class SnapshotRow(
    val index: Int,
    val code: IntArray,
    val fg: IntArray,
    val bg: IntArray,
    val attrs: IntArray,
    val wrapped: Boolean,
    val spans: List<Span>,
) {
    val cols: Int get() = code.size

    fun cellAt(col: Int): Cell = Cell(code[col], fg[col], bg[col], attrs[col])

    /** Which span, if any, covers this column. The last one wins, so a current match beats a selection. */
    fun spanAt(col: Int): SpanKind? = spans.lastOrNull { col >= it.from && col <= it.to }?.kind

    fun text(): String {
        val sb = StringBuilder(code.size)
        for (c in code) if (c != Cell.WIDE_TAIL) sb.appendCodePoint(c)
        return sb.toString()
    }
}

/** One frame, as the renderer sees it. */
class Snapshot(
    val cols: Int,
    val rows: List<SnapshotRow>,
    /** Absolute index of [rows]`[0]`. */
    val top: Int,
    val totalLines: Int,
    /** Row within [rows], or -1 when the cursor is not on screen. */
    val cursorRow: Int,
    val cursorCol: Int,
    val cursorVisible: Boolean,
    val following: Boolean,
    val revision: Long,
    val matchCount: Int,
    val currentMatch: Int,
) {
    /** An absolute position from a row and column of this snapshot. */
    fun posAt(row: Int, col: Int): Pos? {
        val line = rows.getOrNull(row) ?: return null
        return Pos(line.index, col.coerceIn(0, line.cols - 1))
    }

    fun text(): String = rows.joinToString("\n") { it.text().trimEnd() }
}
