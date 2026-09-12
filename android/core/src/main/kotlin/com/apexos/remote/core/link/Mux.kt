package com.apexos.remote.core.link

import com.apexos.remote.core.Frame
import com.apexos.remote.core.FrameChannel
import java.io.Closeable
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException

/**
 * One connection to a machine, with several conversations on it.
 *
 * ## The ordering rule, which is the whole of this file
 *
 * `apex-remoted` answers a `Control` frame with a `Control` frame, and it does
 * so **from a single-threaded loop that blocks while `apex-agentd` thinks**
 * (`serve.rs`: `Frame::Control(line) => { let reply = control(...); send(...) }`).
 * There is no request id on the wire. So the only thing that pairs a reply
 * with its request is that they are in the same order, and a client that let
 * two requests be outstanding at once and matched them by arrival is a client
 * that shows one session's details under another's name.
 *
 * Hence: a strict FIFO of outstanding requests, and replies resolve the head.
 *
 * ## `Open` is answered on channel zero, and sometimes not at all
 *
 * An `Open` gets its answer as a `Control` frame too — the daemon's
 * `{"reply":"attached","id":N}` — which is why it takes a place in the same
 * queue. But the desktop has two failure shapes and only one of them sends a
 * reply:
 *
 * * the daemon said something other than `attached` — `Control(reply)` then
 *   `Close(channel, "")`;
 * * anything else went wrong — **only** `Close(channel, reason)`.
 *
 * A queue that waited for a `Control` per `Open` would therefore hand the
 * *next* request's reply to the failed attach, and every reply after that
 * would be one behind. So a `Close` naming the channel of an unanswered
 * `Open` at the head resolves it, as a failure carrying the reason.
 *
 * ## Why there is no locking around the reader
 *
 * One thread calls [pump] and it is the only caller of [FrameChannel.receive].
 * Everything else it touches is either a concurrent structure or guarded by
 * [lock]. Sending is safe from any thread because the channel serialises it.
 */
class Mux(
    private val channel: FrameChannel,
    private val listener: Listener,
    /**
     * Whether a reply to an `Open` means the channel is now carrying a PTY.
     *
     * A parameter because the daemon answers an `Open` with either
     * `{"reply":"attached",…}` or an error, and a channel recorded as open on
     * an error is a channel whose trailing `Close("")` would then be reported
     * as "the session ended" — a refusal dressed up as a bereavement.
     */
    private val accepted: (ByteArray) -> Boolean = ::attachAccepted,
) : Closeable {
    /** What the owner of a [Mux] is told about the channels on it. */
    interface Listener {
        /** Terminal bytes for an open channel. */
        fun onData(channel: UInt, bytes: ByteArray)

        /** A channel ended. [reason] is empty for an ordinary close. */
        fun onClose(channel: UInt, reason: String)

        /**
         * The connection itself ended.
         *
         * Always called exactly once, whatever ended it, including [close].
         * A caller that reconnects hangs its retry off this.
         */
        fun onDisconnect(cause: Throwable?)
    }

    private class Pending(
        /** Non-null when this is an `Open`, so a `Close` can resolve it. */
        val channel: UInt?,
        val future: CompletableFuture<ByteArray>,
    )

    private val lock = Any()
    private val pending = ArrayDeque<Pending>()
    private val open = HashSet<UInt>()

    @Volatile
    private var closed = false

    @Volatile
    private var disconnected = false

    val machine: String get() = channel.machine

    /**
     * Read frames until the connection ends.
     *
     * Blocking, and meant to own a thread. It returns rather than throwing:
     * the end of a connection is the normal way this finishes, and the cause
     * reaches the caller through [Listener.onDisconnect] where a reconnect
     * loop is already listening.
     */
    fun pump() {
        var cause: Throwable? = null
        try {
            while (!closed) {
                dispatch(channel.receive())
            }
        } catch (e: Throwable) {
            // Not swallowed: `closed` distinguishes "we hung up" from "it
            // died", and only the second is a cause worth reporting.
            if (!closed) cause = e
        } finally {
            finish(cause)
        }
    }

    private fun dispatch(frame: Frame) {
        when (frame) {
            is Frame.Control -> resolveHead(frame.line, null)
            is Frame.Data -> if (frame.channel in openChannels()) listener.onData(frame.channel, frame.bytes)
            is Frame.Close -> {
                // A close that answers an unanswered `Open` at the head is the
                // desktop's "it went wrong and there is no daemon reply" path.
                if (!resolveHead(null, frame)) {
                    synchronized(lock) { open.remove(frame.channel) }
                    listener.onClose(frame.channel, frame.reason)
                }
            }
            // `Open` never arrives at a client: the desktop opens nothing.
            // Ping and Pong are handled inside the channel and never surface.
            else -> Unit
        }
    }

    private fun openChannels(): Set<UInt> = synchronized(lock) { open.toSet() }

    /**
     * Hand a reply to the head of the queue.
     *
     * Returns whether it was consumed, which only matters for the [Frame.Close]
     * case: a close that resolved an `Open` must not also be reported as a
     * channel ending, because no channel was ever opened.
     */
    private fun resolveHead(reply: ByteArray?, close: Frame.Close?): Boolean {
        val head = synchronized(lock) {
            val head = pending.firstOrNull() ?: return false
            if (close != null && head.channel != close.channel) return false
            pending.removeFirst()
            // Recorded on the pump thread and not by the caller, because the
            // desktop sends the scrollback immediately after the reply: the
            // first `Data` frame can arrive before the caller has woken up.
            if (reply != null && head.channel != null && accepted(reply)) open.add(head.channel)
            head
        }
        if (reply != null) {
            head.future.complete(reply)
        } else if (close != null) {
            head.future.completeExceptionally(
                ChannelRefused(
                    if (close.reason.isEmpty()) {
                        "the machine closed the channel without saying why"
                    } else {
                        close.reason
                    },
                ),
            )
        }
        return true
    }

    private fun finish(cause: Throwable?) {
        val waiting = synchronized(lock) {
            if (disconnected) return
            disconnected = true
            val copy = pending.toList()
            pending.clear()
            open.clear()
            copy
        }
        val ended = cause ?: Disconnected("the connection to ${channel.machine} ended")
        // Every outstanding request fails, and none is left to time out on its
        // own. A caller blocked on a reply that can never come is the shape of
        // bug that looks like a slow network for thirty seconds.
        for (p in waiting) p.future.completeExceptionally(ended)
        listener.onDisconnect(cause)
    }

    /**
     * Send one `apex-agentd` request and wait for its reply.
     *
     * The deadline is generous by default because the daemon's own is: a
     * request that reaches a human — a privilege prompt — legitimately takes
     * as long as the person does, and `apex-remoted` waits five minutes for
     * exactly that. A phone that gave up after ten seconds would report a
     * failure while a dialog was still on screen.
     */
    @Throws(Disconnected::class, TimeoutException::class)
    fun request(line: ByteArray, timeoutMs: Long = CONTROL_TIMEOUT_MS): ByteArray {
        val future = enqueue(null)
        channel.send(Frame.Control(line))
        return await(future, timeoutMs)
    }

    fun request(line: String, timeoutMs: Long = CONTROL_TIMEOUT_MS): String =
        request(line.toByteArray(Charsets.UTF_8), timeoutMs).toString(Charsets.UTF_8)

    /**
     * Open a PTY channel, and return the daemon's reply to the `attach`.
     *
     * The reply is returned rather than interpreted: "no such session 4" is
     * the daemon's sentence and belongs on the screen in its own words.
     * A channel is only recorded as open when a reply arrived, so a refused
     * attach leaves nothing behind.
     */
    @Throws(Disconnected::class, ChannelRefused::class, TimeoutException::class)
    fun openChannel(id: UInt, attach: ByteArray, timeoutMs: Long = CONTROL_TIMEOUT_MS): ByteArray {
        require(id != Frame.CONTROL_CHANNEL) { "channel 0 is the control channel" }
        val future = enqueue(id)
        channel.send(Frame.Open(id, attach))
        return await(future, timeoutMs)
    }

    private fun enqueue(forChannel: UInt?): CompletableFuture<ByteArray> {
        val future = CompletableFuture<ByteArray>()
        synchronized(lock) {
            if (disconnected || closed) {
                throw Disconnected("this connection to ${channel.machine} is closed")
            }
            pending.addLast(Pending(forChannel, future))
        }
        return future
    }

    private fun await(future: CompletableFuture<ByteArray>, timeoutMs: Long): ByteArray = try {
        future.get(timeoutMs, TimeUnit.MILLISECONDS)
    } catch (e: java.util.concurrent.ExecutionException) {
        when (val cause = e.cause) {
            is ChannelRefused -> throw cause
            is Disconnected -> throw cause
            else -> throw Disconnected(cause?.message ?: "the connection ended", cause)
        }
    } catch (e: java.util.concurrent.TimeoutException) {
        // The queue is FIFO and this request is no longer in a state anybody
        // can reason about: a reply that arrives later would be handed to the
        // NEXT request. So the connection goes, which is the honest response
        // to having lost track of where in the conversation we are.
        close()
        throw TimeoutException("${channel.machine} did not answer within ${timeoutMs}ms")
    }

    /** Terminal bytes to an open channel. */
    fun send(id: UInt, bytes: ByteArray) {
        if (bytes.isEmpty()) return
        channel.sendData(id, bytes)
    }

    /** Detach: the session keeps running on the machine, which is the point. */
    fun closeChannel(id: UInt, reason: String = "") {
        synchronized(lock) { open.remove(id) }
        runCatching { channel.send(Frame.Close(id, reason)) }
    }

    override fun close() {
        closed = true
        runCatching { channel.close() }
        // Closing the stream is what wakes the pump, which then calls
        // `finish`. But a `Mux` closed before `pump` ever ran would leave
        // callers waiting forever, so the outstanding requests are failed here
        // as well; `finish` is idempotent.
        finish(null)
    }

    companion object {
        /**
         * Five minutes, matching `apex-remoted`'s `CONTROL_TIMEOUT`.
         *
         * Not a guess: the proxy on the far side sets exactly this, for the
         * requests `Request::waits_on_a_human` names. Timing out sooner than
         * the machine does would abandon a request that was still being
         * decided.
         */
        const val CONTROL_TIMEOUT_MS: Long = 300_000
    }
}

/**
 * `apex-agentd` said `attached`, and therefore this channel is a PTY.
 *
 * Read out of the text rather than parsed: key order is not fixed on the wire,
 * so the two halves are looked for independently, and `:core` has no business
 * knowing the reply's full shape.
 */
fun attachAccepted(reply: ByteArray): Boolean {
    val text = reply.toString(Charsets.UTF_8)
    return text.contains("\"reply\"") && text.contains("\"attached\"")
}

/** The connection ended, with or without a reason. */
class Disconnected(message: String, cause: Throwable? = null) : Exception(message, cause)

/** The machine refused to open a channel, in its own words. */
class ChannelRefused(message: String) : Exception(message)
