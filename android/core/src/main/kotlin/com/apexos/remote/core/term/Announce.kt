package com.apexos.remote.core.term

/**
 * What a screen reader is given for one frame, and what it is told out loud.
 *
 * ## Why this is in `:core` and not beside the Canvas
 *
 * `TerminalView` paints the grid with `drawText`, so not one glyph on that
 * screen is a composable and nothing about it reaches the semantics tree by
 * itself. Everything a reader could say therefore has to be *computed* — which
 * line the cursor is on, what the visible buffer reads as, whether the view is
 * live or held in the history, which of the lines that just arrived are worth
 * interrupting somebody for. Those are decisions over a [Snapshot], they are
 * where the mistakes are, and none of them needs Android. So they live here,
 * as ordinary functions with ordinary tests, and the Compose layer does
 * nothing but hand the answers to `Modifier.semantics`.
 *
 * ## The two halves, and why they are not the same node
 *
 * A reader needs two different things from a terminal and they conflict:
 *
 * * **What is on screen**, so it can be explored — swiped line by line, or
 *   read from the top. That is [TerminalReading.text], and it is *pulled*: it
 *   changes silently and is read when the user asks for it.
 * * **What just happened**, so somebody who is not looking learns that the
 *   command they ran has finished. That is [TerminalAnnouncer], and it is
 *   *pushed* through a live region.
 *
 * Putting both on one node is the mistake that makes a terminal unusable with
 * TalkBack on: a live region re-reads the node's whole text whenever it
 * changes, so a node holding the visible buffer would read forty lines aloud
 * every time one character arrived. They are two nodes for that reason.
 */
class TerminalReading(
    /**
     * The visible buffer, one line per row, trailing blanks dropped.
     *
     * This is the node's `text` rather than a `contentDescription` on purpose.
     * TalkBack moves through `text` at the granularity the user picked —
     * character, word, line — and line-by-line movement over the grid is the
     * whole of "reading a terminal" for somebody who cannot see it. A
     * `contentDescription` is one opaque utterance and cannot be navigated.
     */
    val text: String,
    /** The position and liveness of the view, as a sentence. */
    val state: String,
    /** The line the cursor sits on, or `null` when it is not on screen. */
    val cursorLine: String?,
    /**
     * Whether the grid itself is blank.
     *
     * A field rather than `text.isBlank()`, because [text] is never blank:
     * an empty terminal is described in words instead, so the node has
     * something to say. A caller asking "is there output" and getting "yes,
     * there are thirty-three characters of it" would be answered with this
     * class's own fallback.
     */
    val isEmpty: Boolean = false,
) {

    companion object {
        /**
         * A terminal that has not painted yet, which is a real state and not an
         * error: the socket is open, the replay has not arrived.
         *
         * It carries words rather than an empty string because the terminal is
         * clickable — a tap is what raises the keyboard — and a clickable node
         * with nothing to say is announced as "button" and nothing else.
         */
        val EMPTY = TerminalReading(
            text = "Terminal. Nothing on screen yet.",
            state = "Waiting for output.",
            cursorLine = null,
            isEmpty = true,
        )

        /**
         * Read a frame.
         *
         * Blank lines are kept — a terminal's blank lines are layout and a
         * reader that silently closed the gaps would describe a screen nobody
         * is looking at. Trailing blanks on a line go, because a grid pads
         * every row to its full width and eighty spaces is eighty spaces to a
         * speech engine.
         */
        fun of(snapshot: Snapshot): TerminalReading {
            val lines = snapshot.rows.map { speakable(it.text()) }
            val text = lines.joinToString("\n").trimEnd()
            if (text.isBlank()) return EMPTY
            val cursorLine = snapshot.rows.getOrNull(snapshot.cursorRow)
                ?.let { speakable(it.text()) }
                ?.takeIf { it.isNotBlank() }
            return TerminalReading(text = text, state = stateOf(snapshot), cursorLine = cursorLine)
        }

        /**
         * One line, as something a speech engine can pronounce.
         *
         * Control characters are replaced rather than dropped so a column
         * counted from this string still lines up with the grid it came from.
         * They should not be in a cell at all — [Line] fills with spaces and
         * [Terminal] never prints a C0 byte — so this is a floor and not a
         * transformation anything relies on.
         */
        private fun speakable(raw: String): String {
            val out = StringBuilder(raw.length)
            for (ch in raw) out.append(if (ch.code < 0x20 || ch.code == 0x7F) ' ' else ch)
            return out.toString().trimEnd()
        }

        private fun stateOf(snapshot: Snapshot): String {
            val first = snapshot.top + 1
            val last = snapshot.top + snapshot.rows.size
            val where = "Showing lines $first to $last of ${snapshot.totalLines}."
            if (!snapshot.following) {
                // Said first, because it is the thing a person who has scrolled
                // needs to know before anything else: new output is arriving
                // and they are not being shown it.
                return "Scrolled back. $where"
            }
            val cursor = if (snapshot.cursorRow >= 0) {
                "Cursor on line ${snapshot.top + snapshot.cursorRow + 1}, " +
                    "column ${snapshot.cursorCol + 1}."
            } else {
                // A cursor the far end has hidden is the normal state inside a
                // full-screen TUI, and "no cursor" is more honest than pinning
                // it to a corner.
                "No cursor."
            }
            return "Live. $where $cursor"
        }
    }
}

/**
 * What to interrupt somebody with when output arrives.
 *
 * ## Why a policy and not a live region over the buffer
 *
 * The naive version — mark the terminal a polite live region and let Android
 * re-read it on every change — is worse than no accessibility at all. A build
 * running its tests rewrites the screen tens of times a second, and each of
 * those is a fresh interruption that discards the previous utterance. The user
 * hears the first two words of forty announcements and learns nothing.
 *
 * So this decides, per frame, what is genuinely new and worth saying:
 *
 * 1. **Only while the view is following.** Somebody who has scrolled back is
 *    reading; announcing what is arriving below the fold talks over them.
 * 2. **Only lines the cursor has moved past.** The line under the cursor is
 *    still being written — a progress bar, a half-typed command — and reading
 *    it aloud mid-write announces something that was never on screen.
 * 3. **Only lines not already announced**, tracked by absolute line index, so
 *    a repaint of the same row says nothing. This is also what keeps a
 *    full-screen TUI quiet: it rewrites lines in place, and a line that has
 *    already been read is not read again because it moved.
 * 4. **A burst is one utterance.** Ten lines arriving in one frame are one
 *    announcement, capped, with the count of what it left out — rather than
 *    ten announcements of which the user hears the last.
 * 5. **Never the same words twice running.** A spinner that paints
 *    `Building…` on a new line every second is one announcement, not sixty.
 *
 * ## The replay, and why it is silent
 *
 * `apex-agentd` replays its scrollback to every attaching client, so the first
 * frame after an attach holds up to a screenful of history that was already on
 * the far end before this phone arrived. Announcing it would read a stale
 * transcript aloud on every reconnect — including the reconnects the watchdog
 * makes by itself, which the user did not ask for. The first frame therefore
 * only *seeds* the position. [TerminalReading.text] still carries that history,
 * so it is there to be read; it is simply not shouted.
 */
class TerminalAnnouncer(
    /** How many lines one utterance may carry. */
    private val maxLines: Int = 8,
    /** How long it may be. Beyond this a listener has lost the thread anyway. */
    private val maxChars: Int = 240,
) {
    /**
     * The absolute index of the last line already announced, or `null` before
     * the first frame has seeded it.
     */
    private var through: Int? = null

    /**
     * How many lines the terminal held last frame.
     *
     * Kept because it is the only monotonic thing here, and therefore the only
     * honest way to notice a reset. The cursor's own line is not: a TUI that
     * clears the screen and homes the cursor makes "the last finished line" go
     * backwards without a single line having been dropped, and an announcer
     * that read that as a reset would re-seed and then read the TUI's repaint
     * aloud.
     */
    private var total: Int = 0

    private var last: String? = null

    /**
     * Start again, as if nothing had been announced.
     *
     * Called when the terminal is reset — which is what happens on every
     * reconnect, before the replay is fed in.
     */
    fun reset() {
        through = null
        total = 0
        last = null
    }

    /** What to say about this frame, or `null` to say nothing. */
    fun onFrame(snapshot: Snapshot): String? {
        // The last line that is finished: everything above the cursor. With no
        // cursor on screen — a TUI that hid it — the whole visible grid counts,
        // because nothing is going to move past anything.
        val complete = if (snapshot.cursorRow >= 0) {
            snapshot.top + snapshot.cursorRow - 1
        } else {
            snapshot.top + snapshot.rows.size - 1
        }

        val seen = through
        if (seen == null) {
            // First frame. Seed and stay quiet; see the note on the replay.
            through = complete
            total = snapshot.totalLines
            return null
        }
        if (snapshot.totalLines < total) {
            // Lines stopped existing, which a running terminal cannot do: this
            // is `Terminal.reset` and the replay behind it. Re-seed, and forget
            // what was last said so the new session can repeat it.
            through = complete
            total = snapshot.totalLines
            last = null
            return null
        }
        total = snapshot.totalLines
        if (!snapshot.following) return null
        if (complete <= seen) return null

        // Only what is on screen can be read: a burst longer than the viewport
        // has already pushed its oldest lines into the scrollback, and this
        // frame does not carry them. The count of what was missed is said
        // instead, which is the honest form of a summary.
        val fresh = snapshot.rows.filter { it.index in (seen + 1)..complete }
        through = complete

        val words = fresh.map { it.text().trimEnd() }.filter { it.isNotBlank() }
        if (words.isEmpty()) return null

        val spoken = words.takeLast(maxLines)
        val dropped = (complete - seen) - spoken.size
        val body = spoken.joinToString("\n").let {
            if (it.length <= maxChars) it else it.takeLast(maxChars)
        }
        val text = if (dropped > 0) "$dropped earlier lines.\n$body" else body
        if (text == last) return null
        last = text
        return text
    }
}
