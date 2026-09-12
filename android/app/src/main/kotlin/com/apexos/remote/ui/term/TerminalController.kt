package com.apexos.remote.ui.term

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.apexos.remote.core.FrameChannel
import com.apexos.remote.core.link.AttachmentEvent
import com.apexos.remote.core.link.PtyAttachment
import com.apexos.remote.core.term.Keys
import com.apexos.remote.core.term.Mods
import com.apexos.remote.core.term.Terminal
import com.apexos.remote.core.term.Viewport
import com.apexos.remote.core.agent.Agentd
import java.io.Closeable

/** What the screen says about the connection, in one word plus the details. */
sealed class TerminalStatus {
    data class Connecting(val attempt: Int) : TerminalStatus()
    data class Live(val machine: String) : TerminalStatus()
    data class Reconnecting(val inMs: Long, val why: String?) : TerminalStatus()

    /** The session on the machine ended. Nothing to come back to. */
    data class Ended(val reason: String) : TerminalStatus()

    /** The machine refused, in its own words. */
    data class Refused(val reply: String) : TerminalStatus()

    data object Detached : TerminalStatus()
}

/**
 * One attached terminal, owned by the view model and not by a composable.
 *
 * ## Why it lives above the composition
 *
 * Because a rotation destroys and rebuilds every composable, and this holds a
 * socket, a Noise session and a thread. A terminal that re-handshaked every
 * time the phone was turned sideways would cost a fresh 256 KiB replay per
 * rotation — and would lose whatever the user had typed but not sent. The
 * activity declares `configChanges` for orientation as well, so in practice
 * the composition survives; this makes it not matter either way, which is the
 * difference between a property and a coincidence.
 *
 * ## The frame loop, and why the snapshot is pulled rather than pushed
 *
 * `Terminal.feed` runs on the pump thread and can be called hundreds of times
 * a second under a fast `cat`. A terminal that recomposed per feed would spend
 * the whole budget in layout and show fewer frames than one that recomposed
 * sixty times a second. So the pump does nothing but advance
 * `Terminal.revision`, and the screen's `withFrameNanos` loop takes a snapshot
 * only when that number — or something about the viewport — has changed.
 */
class TerminalController(
    val sessionId: Int,
    /** Opens a fresh connection. Called once per attempt, so it must not prompt. */
    connect: () -> FrameChannel,
    cols: Int = 80,
    rows: Int = 24,
) : Closeable {
    val terminal: Terminal = Terminal(cols, rows)
    val viewport: Viewport = Viewport(terminal, rows)

    /** What to show about the connection. Read from the composition. */
    var status: TerminalStatus by mutableStateOf(TerminalStatus.Connecting(1))
        private set

    /** Set when a keystroke was dropped because nothing was attached. */
    var droppedInput: Boolean by mutableStateOf(false)
        private set

    /** How many times this terminal has been attached. Shown, because it explains a redraw. */
    var attachments: Int by mutableStateOf(0)
        private set

    private val attachment = PtyAttachment(
        connect = connect,
        // Rebuilt per attempt, because the phone may have been rotated while
        // it was disconnected and the daemon must be told the shape it is
        // about to paint into.
        attachRequest = { Agentd.attach(sessionId, terminal.cols, terminal.rows) },
        terminal = terminal,
        onEvent = ::onEvent,
    )

    private val thread = Thread({ attachment.run() }, "apex-terminal-$sessionId").apply {
        isDaemon = true
    }

    fun start() {
        if (!thread.isAlive) thread.start()
    }

    private fun onEvent(event: AttachmentEvent) {
        status = when (event) {
            is AttachmentEvent.Connecting -> TerminalStatus.Connecting(event.attempt)
            is AttachmentEvent.Attached -> {
                attachments = event.attempt
                droppedInput = false
                TerminalStatus.Live(event.machine)
            }
            is AttachmentEvent.Lost -> TerminalStatus.Reconnecting(event.inMs, event.cause?.message)
            is AttachmentEvent.Silent ->
                TerminalStatus.Reconnecting(0, "nothing arrived for ${event.forMs / 1000}s")
            is AttachmentEvent.Refused -> TerminalStatus.Refused(event.reply)
            is AttachmentEvent.Ended -> TerminalStatus.Ended(event.reason)
            is AttachmentEvent.WatchdogUnavailable -> return
            AttachmentEvent.Stopped -> TerminalStatus.Detached
        }
    }

    /**
     * Type. Returns whether it went anywhere.
     *
     * A `false` is surfaced rather than swallowed: keystrokes typed while
     * disconnected are dropped on purpose — `PtyAttachment` argues the case —
     * and a screen that said nothing would leave somebody believing they had
     * answered a prompt.
     */
    fun send(bytes: ByteArray): Boolean {
        if (bytes.isEmpty()) return true
        val sent = attachment.send(bytes)
        if (!sent) droppedInput = true
        // Typing is the strongest possible signal that the user wants to see
        // what happens next, so it follows the output again.
        viewport.toBottom()
        return sent
    }

    fun sendText(text: String, mods: Mods = Mods.NONE): Boolean = send(Keys.text(text, mods))

    /** Paste, bracketed when the program asked for it. */
    fun paste(text: String): Boolean = send(Keys.paste(text, terminal.bracketedPaste))

    /**
     * The window changed shape.
     *
     * Both halves in one call, because they fail separately: a terminal that
     * resized itself without telling the program renders the next repaint at
     * the wrong shape, and one that told the program without resizing renders
     * the right repaint into the wrong grid.
     */
    fun resize(cols: Int, rows: Int) {
        if (cols == terminal.cols && rows == terminal.rows) return
        viewport.height = rows
        attachment.resize(sessionId, cols, rows)
    }

    fun clearDropped() {
        droppedInput = false
    }

    /** Detach. The session goes on running on the machine, which is the point. */
    override fun close() {
        attachment.stop()
    }
}
