package com.apexos.remote.core.link

import com.apexos.remote.core.Frame
import com.apexos.remote.core.FrameChannel
import java.io.EOFException
import java.util.concurrent.ConcurrentLinkedQueue
import java.util.concurrent.CountDownLatch
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger

/**
 * A stand-in for `apex-remoted`, with a hand on the plug.
 *
 * ## Why a fake and not the real handshake
 *
 * `RealSessionReconnectTest` does use the real thing — two live `Noise_IK`
 * handshakes over pipes — because a reconnect that never re-handshakes is not
 * a reconnect. What that test cannot do is decide *when* the connection dies.
 * "The socket failed after 400 bytes of a scrollback replay, in the middle of
 * an escape sequence" has to be a case somebody chose, or it is an accident of
 * timing that passes on a machine that is busy and fails on one that is not.
 *
 * ## What it models, from `serve.rs`
 *
 * * A `Control` frame is answered with exactly one `Control` frame, in order,
 *   from a single-threaded loop.
 * * An `Open` is answered with a `Control` frame carrying the daemon's reply,
 *   and then, when that reply was `attached`, the scrollback as `Data` frames
 *   on the opened channel.
 * * A refusal the daemon spoke — "no such session" — is `Control(reply)` and
 *   then `Close(channel, "")`.
 * * A failure the proxy hit is **`Close(channel, reason)` and no `Control` at
 *   all**, which is the ordering trap the multiplexer exists to survive.
 */
class FakeMachine(
    val name: String = "l16",
    /** Replayed to every attaching client, as `apex-agentd`'s scrollback is. */
    private val scrollback: ByteArray = ByteArray(0),
    /** Answers to control requests, by the `cmd` in them. */
    private val control: (String) -> String = { """{"reply":"ok"}""" },
    /** How the `attach` is answered. Default: accepted. */
    private val attachReply: (String) -> String = { """{"reply":"attached","id":1}""" },
    /**
     * Drop the connection after this many bytes of scrollback have been sent.
     * `-1` never drops. Applies to the given attempt numbers only.
     */
    private val dropAfterBytes: Int = -1,
    private val dropOnAttempts: Set<Int> = emptySet(),
    /** Answer an `Open` with only a `Close`, as a proxy failure does. */
    private val closeWithoutReply: String? = null,
    /**
     * How many bytes of scrollback go in one `Data` frame.
     *
     * [CHUNK] models a healthy read. A small number models the other thing a
     * phone actually gets: a congested link where the far end's writes are
     * broken up by the path's MTU and the kernel's window, delivering an
     * escape sequence in three pieces across three frames.
     */
    private val chunkBytes: Int = CHUNK,
    /** Milliseconds between chunks. A slow link, modelled as a slow link. */
    private val chunkDelayMs: Long = 0,
    /**
     * After attaching, send nothing at all and never close — including no
     * pings.
     *
     * The failure mode a close-based test cannot reach: a phone that walked
     * into a lift. TCP does not announce it, the socket does not throw, and
     * everything above looks exactly like a terminal nobody is typing into.
     */
    private val silentAfterAttach: Boolean = false,
    /**
     * Send a `Ping` this often, as `apex-remoted` does every fifteen seconds.
     *
     * Zero for a machine that sends no keepalive at all — which is what makes
     * [silentAfterAttach] a *silent* connection rather than merely an idle
     * one. The two are told apart by exactly this traffic, so a fixture that
     * could not produce it could not test the distinction.
     */
    private val pingEveryMs: Long = 0,
) {
    /** How many `Data` frames have been sent. The throttle's own evidence. */
    val dataFramesSent = AtomicInteger(0)

    /** How many keepalives have gone out. */
    val pingsSent = AtomicInteger(0)
    /** How many connections have been opened. One per handshake, in the real thing. */
    val connections = AtomicInteger(0)

    /** Everything every client has ever sent on a PTY channel. */
    val received = ConcurrentLinkedQueue<ByteArray>()

    /** Every control line any client has sent. */
    val requests = ConcurrentLinkedQueue<String>()

    private val threads = ArrayList<Thread>()

    fun open(): FrameChannel {
        val attempt = connections.incrementAndGet()
        val toClient = LinkedBlockingQueue<Any>()
        val toMachine = LinkedBlockingQueue<Any>()
        val channel = QueueChannel(name, toClient, toMachine)
        val thread = Thread({ serve(attempt, toClient, toMachine) }, "fake-machine-$attempt")
        thread.isDaemon = true
        synchronized(threads) { threads.add(thread) }
        thread.start()
        if (pingEveryMs > 0) {
            // The keepalive, from the machine's side — which is the only side
            // it comes from. A client's own outbound ping does not prove the
            // far end is alive, and a fixture that pinged from the client
            // would be testing the client against itself.
            val keepalive = Thread({
                try {
                    var token = 0L
                    while (true) {
                        Thread.sleep(pingEveryMs)
                        toClient.put(Frame.Ping(token++))
                        pingsSent.incrementAndGet()
                    }
                } catch (_: InterruptedException) {
                    Thread.currentThread().interrupt()
                }
            }, "fake-machine-keepalive-$attempt")
            keepalive.isDaemon = true
            synchronized(threads) { threads.add(keepalive) }
            keepalive.start()
        }
        return channel
    }

    private fun serve(attempt: Int, toClient: LinkedBlockingQueue<Any>, toMachine: LinkedBlockingQueue<Any>) {
        try {
            while (true) {
                val item = toMachine.poll(30, TimeUnit.SECONDS) ?: return
                if (item is Hangup) return
                val frame = item as Frame
                when (frame) {
                    is Frame.Control -> {
                        val line = frame.line.toString(Charsets.UTF_8)
                        requests.add(line)
                        toClient.put(Frame.Control(control(line).toByteArray(Charsets.UTF_8)))
                    }
                    is Frame.Open -> {
                        val request = frame.request.toString(Charsets.UTF_8)
                        requests.add(request)
                        if (closeWithoutReply != null) {
                            // The proxy-failure shape: no `Control` at all.
                            toClient.put(Frame.Close(frame.channel, closeWithoutReply))
                            continue
                        }
                        val reply = attachReply(request)
                        toClient.put(Frame.Control(reply.toByteArray(Charsets.UTF_8)))
                        if (!reply.contains("\"attached\"")) {
                            toClient.put(Frame.Close(frame.channel, ""))
                            continue
                        }
                        if (silentAfterAttach) {
                            // Attached, and then nothing. Not a close, not a
                            // ping: the connection is up as far as every layer
                            // above can tell, and no byte will ever arrive.
                            continue
                        }
                        val drop = attempt in dropOnAttempts && dropAfterBytes >= 0
                        val limit = if (drop) minOf(dropAfterBytes, scrollback.size) else scrollback.size
                        // In pieces, as a real socket delivers it.
                        var at = 0
                        while (at < limit) {
                            val end = minOf(at + chunkBytes, limit)
                            toClient.put(Frame.Data(frame.channel, scrollback.copyOfRange(at, end)))
                            dataFramesSent.incrementAndGet()
                            at = end
                            if (chunkDelayMs > 0) Thread.sleep(chunkDelayMs)
                        }
                        if (drop) {
                            // The plug. A dead socket, not a polite close: the
                            // client must treat an exception from `receive` as
                            // the end of the connection.
                            toClient.put(Hangup)
                            return
                        }
                    }
                    is Frame.Data -> received.add(frame.bytes)
                    is Frame.Close -> Unit
                    else -> Unit
                }
            }
        } catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
        }
    }

    /** Everything a client sent on a PTY channel, joined. */
    fun receivedBytes(): ByteArray {
        val out = java.io.ByteArrayOutputStream()
        for (b in received) out.write(b)
        return out.toByteArray()
    }

    private object Hangup

    /**
     * A [FrameChannel] over two queues.
     *
     * The one rule it shares with the real thing and that matters here:
     * [receive] **throws** at the end of the stream. A fake that returned null
     * would let a client treat a dead connection as a quiet one, which is
     * exactly the bug the reconnect loop exists to avoid.
     */
    private class QueueChannel(
        override val machine: String,
        private val inbound: LinkedBlockingQueue<Any>,
        private val outbound: LinkedBlockingQueue<Any>,
    ) : FrameChannel {
        @Volatile
        private var closed = false

        val closedLatch = CountDownLatch(1)

        override fun send(frame: Frame) {
            if (closed) throw EOFException("this connection to $machine is closed")
            // Encoded and decoded, so the fake exercises the real wire format
            // rather than passing objects around: a frame this client cannot
            // encode must fail here, as it would on a socket.
            outbound.put(Frame.decode(frame.encode()))
        }

        override fun sendData(channelId: UInt, bytes: ByteArray) {
            for (f in Frame.dataFrames(channelId, bytes)) send(f)
        }

        @Volatile
        override var lastFrameNanos: Long? = System.nanoTime()
            private set

        override fun receive(): Frame {
            while (true) {
                if (closed) throw EOFException("this connection to $machine is closed")
                val item = inbound.poll(30, TimeUnit.SECONDS)
                    ?: throw EOFException("$machine said nothing for thirty seconds")
                // Before the keepalive filtering below, and that is the whole
                // point: a ping is traffic even though it never surfaces.
                lastFrameNanos = System.nanoTime()
                if (item is Hangup) {
                    closed = true
                    throw EOFException("the connection to $machine ended")
                }
                val frame = item as Frame
                // Keepalives never surface, as `Session.receive` promises.
                if (frame is Frame.Ping) {
                    send(Frame.Pong(frame.token))
                    continue
                }
                if (frame is Frame.Pong) continue
                return frame
            }
        }

        override fun close() {
            closed = true
            closedLatch.countDown()
            outbound.offer(Hangup)
            inbound.offer(Hangup)
        }
    }

    companion object {
        /** What a real PTY read produces, near enough. */
        const val CHUNK = 4096
    }
}
