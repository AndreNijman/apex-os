package com.rimeos.remote.core.link

import com.rimeos.remote.core.FrameChannel
import com.rimeos.remote.core.term.Terminal

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

    /**
     * The session on the machine ended, so there is nothing to re-attach to.
     *
     * Distinct from [Lost] on purpose: a connection that dropped comes back,
     * and a session that exited does not. The screen should say which.
     */
    data class Ended(val reason: String) : AttachmentEvent()

    /** [PtyAttachment.stop] was called, or the loop gave up. */
    data object Stopped : AttachmentEvent()

    /**
     * Nothing arrived for long enough that the connection is presumed gone.
     *
     * Reported before the [Lost] that follows it, because the two have
     * different causes and a screen that said only "reconnecting" would hide
     * the interesting half: a link that *closed* was closed by something, and
     * a link that went quiet was not.
     */
    data class Silent(val forMs: Long) : AttachmentEvent()

    /**
     * The watchdog could not arm, because the transport does not report when
     * a frame last arrived.
     *
     * Emitted rather than swallowed. A reconnect loop that silently watched
     * nothing is the failure this whole mechanism exists to prevent, and the
     * one shape of it that a passing test looks exactly like.
     */
    data class WatchdogUnavailable(val why: String) : AttachmentEvent()
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
 * calling it is not optional: the daemon sends the scrollback the attach asks
 * for (`Agentd.DEFAULT_REPLAY`, 64 KiB) to every attaching client, so a
 * terminal still holding the last attachment's screen would show the last few
 * hundred lines twice. The reconnect test asserts the recovered screen equals
 * the uninterrupted one, which is exactly the assertion a missing `reset`
 * fails.
 *
 * ## Its own connection, or a channel on the control connection
 *
 * By default each attempt dials a connection of its own ([connect]), which
 * every machine understands. When [shared] is given — a machine whose
 * `rime-remoted` advertises `mux_attach` — each attempt instead opens a
 * CHANNEL on the control connection the app already holds, which costs no
 * handshake at all and is most of why a terminal opens in a blink rather than
 * after a second relay dial. The loop, the reset and the replay are the same
 * either way; what differs is what [stop] closes (only the channel, never a
 * connection somebody else owns) and who watches for silence (the owner of
 * the shared connection already does, so this does not add a second
 * watchdog to the same socket).
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
    /**
     * Treat the connection as gone when no frame — keepalive included — has
     * arrived for this long. Zero disables the watchdog.
     */
    private val silenceTimeoutMs: Long = DEFAULT_SILENCE_MS,
    /** How often the watchdog looks. Injected so a test need not wait a minute. */
    private val watchdogPollMs: Long = DEFAULT_POLL_MS,
    /**
     * The control connection's multiplexer, for a machine that allows
     * terminals on it. When set, [connect] is not used. See the class note.
     */
    private val shared: (() -> Mux)? = null,
) {
    @Volatile
    private var stopped = false

    @Volatile
    private var mux: Mux? = null

    /**
     * The connection the listener answers on, set as soon as it exists.
     *
     * Separate from [mux], and the separation is a bug fix rather than a
     * style. [mux] means "attached", and it governs [send] and [attached] —
     * a keystroke typed before the daemon has accepted the attach must be
     * dropped, which is the documented behaviour above. But it is assigned on
     * *this* thread after `openChannel` returns, and by then the pump thread
     * may already have delivered several `Data` frames: `rime-agentd` sends
     * its scrollback replay the instant it answers `attached`.
     *
     * A listener reading [mux] therefore found `null` for the opening frames
     * and **silently dropped the terminal's answers to them** — which are
     * exactly the frames that carry a TUI's opening `ESC[6n` and `ESC[c`. The
     * TUI then waits forever for a cursor report that was computed and thrown
     * away, which is how a remote terminal "hangs" with nothing in any log.
     *
     * It raced, so it worked on an idle machine and failed on a busy one: the
     * defect surfaced as a twenty-second timeout in the one-byte-per-frame
     * test, but only when the whole suite was running.
     *
     * This is assigned before the pump thread starts, so there is no ordering
     * in which a frame can arrive before it is set.
     */
    @Volatile
    private var wire: Mux? = null

    /** The channel this attachment holds on a [shared] connection, while it holds one. */
    @Volatile
    private var sharedChannel: UInt? = null

    /** Released by [stop], or by the channel or its connection ending. */
    @Volatile
    private var sharedEnded: java.util.concurrent.CountDownLatch? = null

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
                // [wire], not [mux]: the scrollback replay begins before the
                // attaching thread has recorded the connection as attached.
                runCatching { wire?.send(channelId, answers) }
            }
        }

        override fun onClose(channel: UInt, reason: String) {
            if (channel != channelId) return
            // The session on the far side has exited. Not a reconnect: there
            // is nothing left to attach to. The reason is carried out rather
            // than swallowed — "session 4 exited" is a thing to say, and a
            // terminal that simply stopped would look like a network fault.
            stopped = true
            onEvent(AttachmentEvent.Ended(reason))
            runCatching { wire?.close() }
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
        shared?.let { return attemptShared(it) }
        var channel: FrameChannel? = null
        try {
            channel = connect()
            // A screen left while this was connecting. `connect()` is a socket
            // and a Noise handshake and can take seconds on a bad link, and
            // `stop()` during it had nothing to close — so the attempt went on
            // to attach and then sat in `pump.join()` holding a connection the
            // user had walked away from until the far end dropped it.
            if (stopped) {
                runCatching { channel.close() }
                return Outcome.Lost(null)
            }
            // BEFORE the replay, and this line is the reconnect test's target.
            terminal.reset()
            val m = Mux(channel, listener)
            // BEFORE the pump can deliver anything. See [wire].
            wire = m
            // And again, because `stop()` may have run between the check above
            // and this assignment — in which case it saw a null `wire` and
            // closed nothing. Here there is something to close.
            if (stopped) {
                runCatching { m.close() }
                runCatching { channel.close() }
                return Outcome.Lost(null)
            }
            val pump = Thread({ m.pump() }, "rime-pty-$channelId")
            pump.isDaemon = true
            pump.start()
            val reply = try {
                m.openChannel(channelId, attachRequest().toByteArray(Charsets.UTF_8)).toString(Charsets.UTF_8)
            } catch (e: Throwable) {
                runCatching { m.close() }
                pump.join(JOIN_MS)
                return Outcome.Lost(e)
            }
            if (!attachAccepted(reply.toByteArray(Charsets.UTF_8))) {
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
            val watchdog = watch(channel, m)
            // Held here until the connection ends, which is what the pump
            // thread returning means.
            pump.join()
            watchdog?.interrupt()
            mux = null
            wire = null
            return Outcome.Lost(null)
        } catch (e: Throwable) {
            mux = null
            wire = null
            runCatching { channel?.close() }
            return Outcome.Lost(e)
        }
    }

    /**
     * One attempt on a connection this attachment does not own.
     *
     * The same shape as [attempt] with the connection taken out: reset, open
     * the channel with a listener of its own registered BEFORE the `Open`
     * (the scrollback follows the reply immediately, on the pump thread), and
     * then wait — for the session to end, for the connection to drop, or for
     * [stop]. Channel ids come from the multiplexer, so a second terminal on
     * the same connection cannot take this one's.
     */
    private fun attemptShared(host: () -> Mux): Outcome {
        val m = try {
            host()
        } catch (e: Throwable) {
            return Outcome.Lost(e)
        }
        if (stopped) return Outcome.Lost(null)
        terminal.reset()
        val id = m.nextChannelId()
        val ended = java.util.concurrent.CountDownLatch(1)
        val onThis = object : Mux.Listener {
            override fun onData(channel: UInt, bytes: ByteArray) {
                if (channel != id) return
                terminal.feed(bytes)
                val answers = terminal.takeResponses()
                if (answers.isNotEmpty()) runCatching { m.send(id, answers) }
            }

            override fun onClose(channel: UInt, reason: String) {
                if (channel != id) return
                stopped = true
                onEvent(AttachmentEvent.Ended(reason))
                ended.countDown()
            }

            override fun onDisconnect(cause: Throwable?) {
                ended.countDown()
            }
        }
        wire = m
        sharedChannel = id
        sharedEnded = ended
        try {
            if (stopped) return Outcome.Lost(null)
            val reply = try {
                m.openChannel(
                    id,
                    attachRequest().toByteArray(Charsets.UTF_8),
                    SHARED_OPEN_TIMEOUT_MS,
                    onThis,
                ).toString(Charsets.UTF_8)
            } catch (e: Throwable) {
                return Outcome.Lost(e)
            }
            if (!attachAccepted(reply.toByteArray(Charsets.UTF_8))) {
                onEvent(AttachmentEvent.Refused(reply))
                return Outcome.Refused(reply)
            }
            if (stopped) {
                runCatching { m.closeChannel(id) }
                return Outcome.Lost(null)
            }
            mux = m
            attachments++
            onEvent(AttachmentEvent.Attached(m.machine, attempts))
            ended.await()
            return Outcome.Lost(null)
        } finally {
            mux = null
            wire = null
            sharedChannel = null
            sharedEnded = null
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
        val id = sharedChannel ?: channelId
        return runCatching { m.send(id, bytes) }.isSuccess
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
     * The request travels as an ordinary control frame — `rime-agentd` takes
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
        if (shared != null) {
            // Only the CHANNEL. The connection is the control link's, and a
            // terminal that closed it on the way out would take every other
            // screen's connection down with it.
            val m = wire
            val id = sharedChannel
            if (m != null && id != null) runCatching { m.closeChannel(id) }
            sharedEnded?.countDown()
            return
        }
        // [wire] rather than [mux], so a stop during a handshake still closes
        // the connection it is holding open.
        val m = wire
        if (m != null) {
            runCatching { m.closeChannel(channelId) }
            runCatching { m.close() }
        }
    }

    /**
     * Watch for a connection that has gone quiet without going away.
     *
     * ## Why this is not `soTimeout`
     *
     * Because a read deadline cannot tell the two apart. The socket's deadline
     * is deliberately cleared after the handshake — `PairingService.connect`
     * says why, and it is right: a PTY with nobody typing produces no bytes
     * for hours, and a twenty-second deadline on one would end somebody's
     * terminal every twenty seconds of silence. What makes silence
     * *distinguishable* is that the desktop sends a `Ping` every fifteen
     * seconds whether or not anything is happening. So the question is not
     * "has any output arrived" but "has any FRAME arrived", and the answer
     * lives one layer below the multiplexer because that is where pings are
     * answered and discarded.
     *
     * A dead TCP connection does not announce itself: a phone that walks into
     * a lift sends no FIN, and the socket sits in `read` until the kernel
     * gives up, which on Android is hours. Without this, the reconnect loop
     * this class is named for would never run for the most common way a phone
     * loses a connection.
     *
     * Returns the thread so the caller can end it, or `null` when the watchdog
     * did not arm — which is reported as an event rather than passed over.
     */
    private fun watch(channel: FrameChannel, m: Mux): Thread? {
        if (silenceTimeoutMs <= 0) return null
        if (channel.lastFrameNanos == null) {
            onEvent(
                AttachmentEvent.WatchdogUnavailable(
                    "this transport does not report when a frame last arrived, so a connection " +
                        "that goes quiet without closing will not be noticed",
                ),
            )
            return null
        }
        val thread = Thread({
            try {
                while (!stopped) {
                    Thread.sleep(watchdogPollMs)
                    val last = channel.lastFrameNanos ?: return@Thread
                    val idleMs = (System.nanoTime() - last) / 1_000_000
                    if (idleMs >= silenceTimeoutMs) {
                        onEvent(AttachmentEvent.Silent(idleMs))
                        // Closing is what wakes the pump, which ends the
                        // attempt and puts the reconnect loop back in charge.
                        // Nothing here decides to reconnect; it only decides
                        // that this connection is over.
                        runCatching { m.close() }
                        return@Thread
                    }
                }
            } catch (_: InterruptedException) {
                Thread.currentThread().interrupt()
            }
        }, "rime-pty-watchdog-$channelId")
        thread.isDaemon = true
        thread.start()
        return thread
    }

    private companion object {
        const val DEFAULT_CHANNEL: UInt = 1u
        const val JOIN_MS: Long = 2_000

        /**
         * Forty-five seconds: three of the desktop's fifteen-second pings.
         *
         * One missed ping is a busy machine or a slow hop. Three is a
         * connection. Waiting for three costs at most forty-five seconds of a
         * terminal that is already dead, and firing after one would reconnect
         * a working session every time the far end paused under load — which
         * costs a fresh handshake and a scrollback replay on a phone's data
         * allowance.
         */
        const val DEFAULT_SILENCE_MS: Long = 45_000

        /** Often enough that the answer is within a ping of the truth. */
        const val DEFAULT_POLL_MS: Long = 2_000

        /**
         * A resize is not a request that waits on a human, so it does not get
         * the human-sized deadline. If it has not been answered in this long
         * the connection is in trouble and the reconnect path is the right
         * place to be.
         */
        const val RESIZE_TIMEOUT_MS: Long = 15_000

        /**
         * How long an attach on a shared connection may take to be answered.
         *
         * Not the five-minute control deadline: an attach does not wait on a
         * human, and on a shared connection a timeout closes the connection
         * every other screen is using (the multiplexer's FIFO has lost its
         * place). Thirty seconds is a machine that is not answering, and the
         * reconnect path is the right place for that.
         */
        const val SHARED_OPEN_TIMEOUT_MS: Long = 30_000
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
