package com.apexos.remote.core.link

import com.apexos.remote.core.FrameChannel
import com.apexos.remote.core.term.Terminal

/** What an attachment tells whoever is watching it. */
sealed class AttachmentEvent {
    /** A connection is being made. [attempt] is 1 for the first. */
    data class Connecting(val attempt: Int) : AttachmentEvent()

    /** The PTY is attached and bytes are flowing. */
    data class Attached(val machine: String, val attempt: Int) : AttachmentEvent()

    /**
     * The machine refused, in its own words.
     *
     * Terminal: there is no session to come back to, so the loop stops. "No
     * such session 4" does not get better by being retried.
     */
    data class Refused(val reply: String) : AttachmentEvent()

    /** The connection dropped and another attempt is coming in [inMs]. */
    data class Lost(val cause: Throwable?, val inMs: Long) : AttachmentEvent()

    /** [PtyAttachment.stop] was called, or the loop gave up. */
    data object Stopped : AttachmentEvent()
}

/**
 * One terminal, attached across however many connections that takes.
 *
 * ## Reconnecting is re-attaching, and it is not resuming
 *
 * There is nothing to resume. A Noise transport has no resumption and the
 * nonces are counters, so a dropped connection is gone; what comes back is a
 * **new handshake, a new session, and a new `attach`**. The daemon does not
 * mind — a session outlives its attachers, which is the entire point of the
 * viewport — and it replays its scrollback to whoever attaches.
 *
 * That replay is why [Terminal.reset] is called before each attempt, and
 * calling it is not optional: the daemon sends up to 256 KiB of history to
 * every attaching client, so a terminal still holding the last attachment's
 * screen would show the last few hundred lines twice. The reconnect test
 * asserts the recovered screen equals the uninterrupted one, which is exactly
 * the assertion a missing `reset` fails.
 *
 * ## Keystrokes typed while disconnected are dropped
 *
 * Deliberately, and it is the less obvious choice. A queue would feel better
 * for a second and then deliver a `y`, or a `Ctrl-C`, into whatever the agent
 * happened to be doing thirty seconds later. Input to a terminal is not a
 * message; it is an event with a moment attached, and a moment that has passed
 * is not worth replaying. [send] answers `false` so the screen can say so.
 */
class PtyAttachment(
    /** Opens a fresh connection. Called once per attempt; it may throw. */
    private val connect: () -> FrameChannel,
    /** The `attach` request, rebuilt per attempt because the size may have changed. */
    private val attachRequest: () -> String,
    /** Where the bytes land. Reset before every attempt. */
    val terminal: Terminal,
    private val channelId: UInt = DEFAULT_CHANNEL,
    private val onEvent: (AttachmentEvent) -> Unit = {},
    /** How long to wait before attempt [n]. Injected so a test does not sleep. */
    private val backoffMs: (Int) -> Long = ::defaultBackoff,
    private val sleep: (Long) -> Unit = { if (it > 0) Thread.sleep(it) },
    /** Give up after this many consecutive failures. Zero means never. */
    private val maxAttempts: Int = 0,
) {
    @Volatile
    private var stopped = false

    @Volatile
    private var mux: Mux? = null

    /** How many connection attempts have been made, successful or not. */
    @Volatile
    var attempts: Int = 0
        private set

    /** How many times a PTY was successfully attached. */
    @Volatile
    var attachments: Int = 0
        private set

    /** True between a successful attach and the connection ending. */
    val attached: Boolean get() = mux != null

    private val listener = object : Mux.Listener {
        override fun onData(channel: UInt, bytes: ByteArray) {
            if (channel != channelId) return
            terminal.feed(bytes)
            // The terminal's answers go straight back up the same channel.
            // Nothing else can send them: they are answers to questions the
            // program asked microseconds ago, and a layer that batched them
            // would be a layer that made a TUI wait.
            val answers = terminal.takeResponses()
            if (answers.isNotEmpty()) {
                runCatching { mux?.send(channelId, answers) }
            }
        }

        override fun onClose(channel: UInt, reason: String) {
            if (channel != channelId) return
            // The session on the far side has exited. Not a reconnect: there
            // is nothing left to attach to.
            stopped = true
            runCatching { mux?.close() }
        }

        override fun onDisconnect(cause: Throwable?) {
            mux = null
        }
    }

    /**
     * Attach, and keep attaching, until [stop] or a refusal.
     *
     * Blocking: it owns the thread it is called on. Everything it does that
     * can block is a socket read, which [stop] ends by closing the socket.
     */
    fun run() {
        while (!stopped) {
            attempts++
            onEvent(AttachmentEvent.Connecting(attempts))
            val outcome = attempt()
            if (stopped || outcome is Outcome.Refused) break
            if (maxAttempts > 0 && attempts >= maxAttempts) break
            val wait = backoffMs(attempts)
            onEvent(AttachmentEvent.Lost((outcome as? Outcome.Lost)?.cause, wait))
            if (stopped) break
            sleep(wait)
        }
        onEvent(AttachmentEvent.Stopped)
    }

    private sealed class Outcome {
        data class Lost(val cause: Throwable?) : Outcome()
        data class Refused(val reply: String) : Outcome()
    }

    private fun attempt(): Outcome {
        var channel: FrameChannel? = null
        try {
            channel = connect()
            // BEFORE the replay, and this line is the reconnect test's target.
            terminal.reset()
            val m = Mux(channel, listener)
            val pump = Thread({ m.pump() }, "apex-pty-$channelId")
            pump.isDaemon = true
            pump.start()
            val reply = try {
                m.openChannel(channelId, attachRequest().toByteArray(Charsets.UTF_8)).toString(Charsets.UTF_8)
            } catch (e: Throwable) {
                runCatching { m.close() }
                pump.join(JOIN_MS)
                return Outcome.Lost(e)
            }
            if (!isAttached(reply)) {
                // The daemon answered something that is not `attached`: no
                // such session, or one that has already exited. Its words go
                // to the screen and the loop stops, because retrying a session
                // that does not exist produces the same answer forever.
                runCatching { m.close() }
                pump.join(JOIN_MS)
                onEvent(AttachmentEvent.Refused(reply))
                return Outcome.Refused(reply)
            }
            mux = m
            attachments++
            onEvent(AttachmentEvent.Attached(m.machine, attempts))
            // Held here until the connection ends, which is what the pump
            // thread returning means.
            pump.join()
            mux = null
            return Outcome.Lost(null)
        } catch (e: Throwable) {
            mux = null
            runCatching { channel?.close() }
            return Outcome.Lost(e)
        }
    }

    /**
     * Send bytes to the PTY. Returns whether there was a connection to send on.
     *
     * `false` is a real answer and not an error: see the note about dropped
     * keystrokes above.
     */
    fun send(bytes: ByteArray): Boolean {
        val m = mux ?: return false
        return runCatching { m.send(channelId, bytes) }.isSuccess
    }

    /**
     * Tell the far end the window changed, and change this end to match.
     *
     * Two things that can fail separately, which is why they are one call
     * here: a terminal that resized itself without telling the program would
     * render the next repaint at the wrong shape, and one that told the
     * program without resizing would render the right repaint into the wrong
     * grid.
     *
     * The request travels as an ordinary control frame — `apex-agentd` takes
     * a resize on its own short-lived connection precisely because
     * multiplexing control into a byte stream that must stay transparent to
     * arbitrary terminal output is how somebody's editor gets corrupted.
     */
    fun resize(sessionId: Int, cols: Int, rows: Int): Boolean {
        terminal.resize(cols, rows)
        val m = mux ?: return false
        return runCatching {
            m.request("""{"cmd":"resize","id":$sessionId,"cols":$cols,"rows":$rows}""", RESIZE_TIMEOUT_MS)
        }.isSuccess
    }

    /** Detach and stop reconnecting. The session goes on running on the machine. */
    fun stop() {
        stopped = true
        val m = mux
        if (m != null) {
            runCatching { m.closeChannel(channelId) }
            runCatching { m.close() }
        }
    }

    private companion object {
        const val DEFAULT_CHANNEL: UInt = 1u
        const val JOIN_MS: Long = 2_000

        /**
         * A resize is not a request that waits on a human, so it does not get
         * the human-sized deadline. If it has not been answered in this long
         * the connection is in trouble and the reconnect path is the right
         * place to be.
         */
        const val RESIZE_TIMEOUT_MS: Long = 15_000

        fun isAttached(reply: String): Boolean =
            // Read out of the text rather than parsed, because this is `:core`
            // and the reply's full shape belongs to the layer that renders it.
            // Key order is not fixed on the wire, so the two halves are looked
            // for independently.
            reply.contains("\"reply\"") && reply.contains("\"attached\"")
    }
}

/**
 * How long to wait before attempt [n].
 *
 * Exponential to sixteen seconds, and then flat. A phone in a lift or on a
 * train reconnects constantly; a backoff that kept doubling would have it
 * sulking for four minutes after a tunnel. Flat at sixteen seconds is
 * cheap enough for the battery and fast enough that coming back out of the
 * tunnel is not noticed.
 *
 * No jitter, deliberately: jitter exists to stop a thousand clients
 * retrying in lockstep, and one phone is not a thundering herd. It would
 * also make every test of this non-deterministic.
 */
fun defaultBackoff(n: Int): Long = when {
    n <= 1 -> 0L
    n == 2 -> 500L
    n == 3 -> 1_000L
    n == 4 -> 2_000L
    n == 5 -> 4_000L
    n == 6 -> 8_000L
    else -> 16_000L
}
