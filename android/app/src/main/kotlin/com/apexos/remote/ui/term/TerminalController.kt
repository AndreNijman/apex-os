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
import java.util.concurrent.Executors

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

    /**
     * Whether [start] has run.
     *
     * Not `thread.isAlive`, which is the bug that reads like the fix. A thread
     * that has FINISHED — the session ended, or the machine refused — is also
     * not alive, and `Thread.start()` on one throws
     * `IllegalThreadStateException`. `DisposableEffect(controller)` re-runs on
     * an activity recreation that keeps the view model, so the second start is
     * not hypothetical.
     */
    @Volatile
    private var started = false

    /**
     * Everything that touches the socket, on one thread that is not the main
     * one.
     *
     * ## Why this exists at all
     *
     * Android installs `StrictMode.enableDeathOnNetwork()` for every app
     * targeting API 11 or later. It is not a flag, and a debug build does not
     * relax it. So a write to a socket from the main thread does not run
     * slowly — it throws `NetworkOnMainThreadException` and the process dies.
     *
     * Every path here reaches a socket: `send` goes through `Mux.send` to
     * `Session.send` to `OutputStream.write`; `resize` goes through
     * `Mux.request`, which is a **blocking round trip** with a fifteen-second
     * deadline and would be an ANR even if it did not throw; `close` writes a
     * `Close` frame. A tap on the accessory row would have killed the app on
     * the first keystroke, on a real phone, every time — and no JVM test and
     * no `assembleDebug` can see it.
     *
     * **Single**-threaded, and that is not an arbitrary pool size: keystrokes
     * are a stream and must reach the PTY in the order they were typed. A pool
     * would deliver `ls` as `sl` under load.
     */
    private val io = Executors.newSingleThreadExecutor { r ->
        Thread(r, "apex-terminal-io-$sessionId").apply { isDaemon = true }
    }

    fun start() {
        if (started) return
        started = true
        thread.start()
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
        // Answered here and now, because the caller is a button that has to
        // decide what to show. `attached` is a volatile read and touches no
        // socket; the write itself goes to the io thread.
        val attached = attachment.attached
        if (!attached) {
            droppedInput = true
        } else {
            submit { if (!attachment.send(bytes)) droppedInput = true }
        }
        // In memory, so it stays inline: typing is the strongest possible
        // signal that the user wants to see what happens next.
        viewport.toBottom()
        return attached
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
        // The in-memory halves inline, so the very next frame is drawn at the
        // right shape; only the request to the machine goes to the io thread.
        // `Mux.request` blocks for up to fifteen seconds waiting for the
        // daemon's reply, and doing that from `onSizeChanged` would freeze the
        // frame a rotation is in the middle of.
        viewport.height = rows
        terminal.resize(cols, rows)
        submit { attachment.resize(sessionId, cols, rows) }
    }

    fun clearDropped() {
        droppedInput = false
    }

    /**
     * Detach. The session goes on running on the machine, which is the point.
     *
     * `stop()` writes a `Close` frame and shuts a socket, so it goes to the io
     * thread like everything else — libcore's socket close trips BlockGuard
     * too, and a screen closing itself is exactly when nobody is watching for
     * the crash.
     */
    override fun close() {
        submit { attachment.stop() }
        io.shutdown()
    }

    /**
     * Run something on the io thread, or drop it if the terminal is gone.
     *
     * `RejectedExecutionException` after [close] is the ordinary case, not an
     * error: a resize can arrive from a layout pass that was already in flight
     * when the screen went away, and there is nothing left to send it to.
     */
    private fun submit(work: () -> Unit) {
        runCatching { io.execute { runCatching { work() } } }
    }
}
