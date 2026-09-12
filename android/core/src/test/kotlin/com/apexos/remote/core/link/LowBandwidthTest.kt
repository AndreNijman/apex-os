package com.apexos.remote.core.link

import com.apexos.remote.core.term.Ansi
import com.apexos.remote.core.term.Terminal
import com.apexos.remote.core.term.TuiFixtures
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertNotEquals
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.Timeout
import java.util.concurrent.ConcurrentLinkedQueue
import java.util.concurrent.TimeUnit

/**
 * P1-055: "low-bandwidth and reconnect behaviour is tested."
 *
 * ## The way this criterion is usually met, and why that is worthless
 *
 * By connecting, disconnecting on purpose, connecting again, and asserting it
 * worked. `ReconnectTest` already does the honest version of that — a socket
 * killed mid-replay, with an assertion that the recovered screen equals the
 * uninterrupted one, and a companion test that *fails* if no drop occurred.
 *
 * What none of it reaches is the way a phone actually loses a connection.
 * A phone does not get a `FIN`. It goes into a lift, or off the end of a
 * platform, and the socket stays open forever: `soTimeout` is deliberately
 * zero after the handshake — `PairingService.connect` explains why, and it is
 * right — so `read` blocks until the kernel's own keepalive gives up, which on
 * Android is hours. Every layer above sees exactly what an idle terminal looks
 * like. **A reconnect test that passes because the connection was politely
 * closed has proved nothing about the case that matters.**
 *
 * So this file has two halves, and the second is the one the criterion is
 * really asking for:
 *
 * 1. **Low bandwidth.** The same recorded TUI streams, delivered in tiny
 *    frames with a delay between them — escape sequences split across the
 *    wire, as a congested path splits them — and the screen must come out
 *    identical to the one an unthrottled link produces. The throttle is
 *    asserted to have applied, because a test of a throttle that was not
 *    applied is a test of nothing.
 * 2. **Silence.** A machine that attaches and then says nothing at all,
 *    forever, without closing. The attachment must notice and reconnect.
 */
class LowBandwidthTest {
    private fun fixture(): ByteArray = TuiFixtures.read(TuiFixtures.CLAUDE_SESSION)

    private fun screenOf(t: Terminal): String = t.read { screen ->
        buildString {
            for (r in 0 until screen.rows) {
                append(screen.row(r).text().trimEnd())
                append('\n')
            }
        }
    }

    /** The same bytes through a terminal with nothing in the way. */
    private fun uninterrupted(bytes: ByteArray): Terminal =
        Terminal(80, 24).also { it.feed(bytes) }

    private fun waitUntil(timeoutMs: Long, what: String, condition: () -> Boolean) {
        val deadline = System.currentTimeMillis() + timeoutMs
        while (System.currentTimeMillis() < deadline) {
            if (condition()) return
            Thread.sleep(5)
        }
        throw AssertionError("timed out after ${timeoutMs}ms waiting for: $what")
    }

    // ---- low bandwidth --------------------------------------------------

    @Test
    @Timeout(60)
    fun `a trickled stream paints the same screen as an unthrottled one`() {
        val bytes = fixture()
        val machine = FakeMachine(
            scrollback = bytes,
            // Seventeen bytes: not a round number, so a sequence is as likely
            // to be cut in its middle as at a boundary, and no escape sequence
            // in the fixture happens to align with the frame size.
            chunkBytes = 17,
            chunkDelayMs = 1,
        )
        val terminal = Terminal(80, 24)
        val events = ConcurrentLinkedQueue<AttachmentEvent>()
        val attachment = PtyAttachment(
            connect = { machine.open() },
            attachRequest = { """{"cmd":"attach","id":1,"cols":80,"rows":24}""" },
            terminal = terminal,
            onEvent = { events.add(it) },
            maxAttempts = 1,
            silenceTimeoutMs = 0,
        )
        val thread = Thread({ attachment.run() }, "trickle")
        thread.isDaemon = true
        thread.start()

        waitUntil(45_000, "the whole trickled stream to arrive") {
            screenOf(terminal) == screenOf(uninterrupted(bytes))
        }
        attachment.stop()
        thread.join(5_000)

        // The throttle really applied. Without this the test would pass just
        // as well against a link that delivered everything in one frame, and
        // would then be a test of nothing.
        val frames = machine.dataFramesSent.get()
        val unthrottled = (bytes.size + FakeMachine.CHUNK - 1) / FakeMachine.CHUNK
        assertTrue(
            frames > unthrottled * 10,
            "the stream was not actually trickled: $frames frames for ${bytes.size} bytes, " +
                "where an unthrottled link would have used $unthrottled",
        )
        assertEquals(1, attachment.attempts, "a slow link is not a broken one; it must not reconnect")
        assertTrue(events.none { it is AttachmentEvent.Lost }, "a slow link was reported as a lost one")
    }

    @Test
    @Timeout(60)
    fun `the terminal still answers what the TUI asked, one byte at a time`() {
        // The property the whole attachment exists for: a TUI's questions are
        // answered from the pump thread microseconds after they arrive. A
        // one-byte-per-frame link splits `ESC [ 6 n` across four frames, and a
        // parser that answered per frame rather than per sequence would send
        // nothing — the TUI would then sit waiting for a cursor report that
        // never comes, which is how a remote terminal "hangs" with no error.
        // Through `Ansi`, never as a raw 0x1b in the source: `Ansi.kt` records
        // what a literal escape character costs — invisible in a diff, and an
        // empty one makes every assertion in a file test a string with no
        // escape in it, which passes.
        val ask = ("hello" + Ansi.csi("6n") + Ansi.csi("c")).toByteArray(Charsets.UTF_8)
        val machine = FakeMachine(scrollback = ask, chunkBytes = 1, chunkDelayMs = 0)
        val terminal = Terminal(80, 24)
        val attachment = PtyAttachment(
            connect = { machine.open() },
            attachRequest = { """{"cmd":"attach","id":1,"cols":80,"rows":24}""" },
            terminal = terminal,
            maxAttempts = 1,
            silenceTimeoutMs = 0,
        )
        val thread = Thread({ attachment.run() }, "trickle-answers")
        thread.isDaemon = true
        thread.start()

        waitUntil(20_000, "the cursor report and the device attributes to come back") {
            val sent = machine.receivedBytes().toString(Charsets.UTF_8)
            sent.contains(Ansi.csi("1;6R")) && sent.contains(Ansi.csi("?6"))
        }
        attachment.stop()
        thread.join(5_000)

        assertEquals(ask.size, machine.dataFramesSent.get(), "the stream was not one byte per frame")
    }

    @Test
    @Timeout(60)
    fun `the opening questions are answered even when the replay beats the attach`() {
        // A REGRESSION TEST FOR A DEFECT THIS FILE FOUND.
        //
        // `apex-agentd` sends up to 256 KiB of scrollback the instant it
        // answers `attached`, so the pump thread can be several `Data` frames
        // deep before the attaching thread has finished recording the
        // connection. The listener read the field that means "attached", found
        // null for those opening frames, and silently dropped the terminal's
        // answers to them — which are precisely the frames carrying a TUI's
        // `ESC[6n` and `ESC[c`. The TUI then waits forever for a cursor report
        // that was computed and thrown away: a remote terminal that hangs with
        // nothing in any log.
        //
        // It raced, so it passed on an idle machine and failed as a
        // twenty-second timeout only once the whole suite was running. A test
        // that just attached twenty-five times would go on passing on a quiet
        // box — so the ordering is FORCED rather than hoped for: the channel
        // holds the attaching thread inside `send(Open)` while the machine
        // answers and replays. That is the real ordering on any link whose
        // round trip is shorter than a thread wake-up, which is every LAN.
        val ask = ("hi" + Ansi.csi("6n")).toByteArray(Charsets.UTF_8)
        val machine = FakeMachine(scrollback = ask)
        val attachment = PtyAttachment(
            connect = {
                val real = machine.open()
                object : com.apexos.remote.core.FrameChannel by real {
                    override fun send(frame: com.apexos.remote.core.Frame) {
                        real.send(frame)
                        // Forwarded, THEN held — so the machine's reply and
                        // its replay both reach the pump before the attaching
                        // thread returns from `openChannel`.
                        if (frame is com.apexos.remote.core.Frame.Open) Thread.sleep(250)
                    }
                }
            },
            attachRequest = { """{"cmd":"attach","id":1,"cols":80,"rows":24}""" },
            terminal = Terminal(80, 24),
            maxAttempts = 1,
            silenceTimeoutMs = 0,
        )
        val thread = Thread({ attachment.run() }, "replay-beats-attach")
        thread.isDaemon = true
        thread.start()
        try {
            waitUntil(15_000, "the cursor report the TUI asked for during the replay") {
                machine.receivedBytes().toString(Charsets.UTF_8).contains(Ansi.csi("1;3R"))
            }
        } finally {
            attachment.stop()
            thread.join(5_000)
        }
    }

    // ---- silence --------------------------------------------------------

    @Test
    @Timeout(60)
    fun `a screen left while connecting does not leave a connection behind`() {
        // `connect()` is a socket and a Noise handshake and takes seconds on a
        // bad link. A `stop()` during it had nothing to close — `wire` is null
        // until the Mux exists — so the attempt went on to attach and then sat
        // in `pump.join()` holding a connection the user had walked away from,
        // until the far end dropped it.
        val machine = FakeMachine(scrollback = "late\r\n".toByteArray(Charsets.UTF_8))
        val connecting = java.util.concurrent.CountDownLatch(1)
        val attachment = PtyAttachment(
            connect = {
                connecting.countDown()
                Thread.sleep(300)
                machine.open()
            },
            attachRequest = { """{"cmd":"attach","id":1,"cols":80,"rows":24}""" },
            terminal = Terminal(80, 24),
            maxAttempts = 1,
            silenceTimeoutMs = 0,
        )
        val thread = Thread({ attachment.run() }, "left-while-connecting")
        thread.isDaemon = true
        thread.start()
        assertTrue(connecting.await(10, TimeUnit.SECONDS), "the connect never started")
        attachment.stop()

        // The run loop must come back promptly rather than settling in for the
        // session it was told not to want.
        thread.join(10_000)
        assertFalse(thread.isAlive, "the attachment stayed attached after it was stopped")
        assertEquals(0, attachment.attachments, "a stopped attachment attached anyway")
        assertFalse(attachment.attached)
    }

    @Test
    @Timeout(60)
    fun `a connection that goes quiet without closing is noticed and re-attached`() {
        // The lift. The first connection attaches and then says nothing at
        // all — no data, no close, no ping — and the second one behaves. A
        // build with no watchdog waits here until this test's own deadline,
        // because that is precisely what the phone would do.
        val bytes = "after the lift\r\n".toByteArray(Charsets.UTF_8)
        val attempt = java.util.concurrent.atomic.AtomicInteger(0)
        val silent = FakeMachine(scrollback = bytes, silentAfterAttach = true)
        val talkative = FakeMachine(scrollback = bytes)
        val events = ConcurrentLinkedQueue<AttachmentEvent>()
        val terminal = Terminal(80, 24)
        val attachment = PtyAttachment(
            connect = { if (attempt.getAndIncrement() == 0) silent.open() else talkative.open() },
            attachRequest = { """{"cmd":"attach","id":1,"cols":80,"rows":24}""" },
            terminal = terminal,
            onEvent = { events.add(it) },
            backoffMs = { 0 },
            maxAttempts = 2,
            // Short, because the test should take a second and not forty-five.
            // The ratio to the poll is what the shipped constants have.
            silenceTimeoutMs = 300,
            watchdogPollMs = 25,
        )
        val thread = Thread({ attachment.run() }, "silence")
        thread.isDaemon = true
        thread.start()

        waitUntil(30_000, "the terminal to show output from the second connection") {
            screenOf(terminal).contains("after the lift")
        }
        attachment.stop()
        thread.join(5_000)

        assertEquals(2, attachment.attempts, "the silent connection was never abandoned")
        assertEquals(2, attachment.attachments)
        val silence = events.filterIsInstance<AttachmentEvent.Silent>()
        assertEquals(1, silence.size, "the silence was not reported: ${events.toList()}")
        assertTrue(
            silence.first().forMs >= 300,
            "the watchdog fired after only ${silence.first().forMs}ms of silence",
        )
        assertTrue(
            events.none { it is AttachmentEvent.WatchdogUnavailable },
            "the watchdog did not arm, so nothing here was watched: ${events.toList()}",
        )
    }

    @Test
    @Timeout(60)
    fun `a quiet connection that is still pinged is left alone`() {
        // The other half, and the one that stops the watchdog from being a
        // timer that ends every idle terminal. `apex-remoted` pings every
        // fifteen seconds; a terminal nobody is typing into has no other
        // traffic at all, so a watchdog that counted only PTY output would
        // reconnect a perfectly healthy session every forty-five seconds.
        //
        // The machine here attaches, sends no data, never closes — and pings.
        // Note which side the ping comes from: a client's own outbound ping
        // proves nothing about the far end, and an earlier version of this
        // test pinged from the client and watched the attachment reconnect
        // three times while claiming to prove the opposite.
        val machine = FakeMachine(silentAfterAttach = true, pingEveryMs = 20)
        val events = ConcurrentLinkedQueue<AttachmentEvent>()
        val attachment = PtyAttachment(
            connect = { machine.open() },
            attachRequest = { """{"cmd":"attach","id":1,"cols":80,"rows":24}""" },
            terminal = Terminal(80, 24),
            onEvent = { events.add(it) },
            maxAttempts = 3,
            silenceTimeoutMs = 300,
            watchdogPollMs = 25,
        )
        val thread = Thread({ attachment.run() }, "kept-alive")
        thread.isDaemon = true
        thread.start()

        waitUntil(20_000, "enough keepalives to outlast the silence timeout several times") {
            machine.pingsSent.get() >= 60
        }
        val stillOne = attachment.attempts
        attachment.stop()
        thread.join(5_000)

        assertEquals(
            1,
            stillOne,
            "a healthy but idle terminal was reconnected: ${events.toList()}",
        )
        assertTrue(
            events.none { it is AttachmentEvent.Silent },
            "a pinged connection was called silent",
        )
    }

    @Test
    @Timeout(60)
    fun `a transport that cannot report liveness says so rather than being watched in name only`() {
        // The shape of failure this whole mechanism is vulnerable to: a
        // watchdog armed over a channel that never updates its clock would
        // either fire immediately or never, and either way nothing would be
        // watching. `FrameChannel.lastFrameNanos` answers null for such a
        // transport and the attachment reports it instead of pretending.
        val inner = FakeMachine(scrollback = "x".toByteArray())
        val events = ConcurrentLinkedQueue<AttachmentEvent>()
        val attachment = PtyAttachment(
            connect = {
                val real = inner.open()
                object : com.apexos.remote.core.FrameChannel by real {
                    override val lastFrameNanos: Long? get() = null
                }
            },
            attachRequest = { """{"cmd":"attach","id":1,"cols":80,"rows":24}""" },
            terminal = Terminal(80, 24),
            onEvent = { events.add(it) },
            maxAttempts = 1,
            silenceTimeoutMs = 300,
            watchdogPollMs = 25,
        )
        val thread = Thread({ attachment.run() }, "unwatchable")
        thread.isDaemon = true
        thread.start()
        waitUntil(20_000, "the attachment to report that it cannot watch this transport") {
            events.any { it is AttachmentEvent.WatchdogUnavailable }
        }
        attachment.stop()
        thread.join(5_000)
        assertFalse(
            events.any { it is AttachmentEvent.Silent },
            "a watchdog that could not arm nevertheless declared a connection silent",
        )
    }

    @Test
    @Timeout(120)
    fun `a link that trickles AND drops still recovers the same screen`() {
        // Both at once, which is what a train actually does: the bytes come
        // slowly and then stop. The drop lands 900 bytes into the replay,
        // inside the fixture's opening escape sequences, and the recovered
        // screen must equal the uninterrupted one — which it can only do if
        // the terminal was reset before the second replay AND the split
        // sequences were reassembled across frames.
        val bytes = fixture()
        val machine = FakeMachine(
            scrollback = bytes,
            chunkBytes = 13,
            dropAfterBytes = 900,
            dropOnAttempts = setOf(1),
        )
        val terminal = Terminal(80, 24)
        val events = ConcurrentLinkedQueue<AttachmentEvent>()
        val attachment = PtyAttachment(
            connect = { machine.open() },
            attachRequest = { """{"cmd":"attach","id":1,"cols":80,"rows":24}""" },
            terminal = terminal,
            onEvent = { events.add(it) },
            backoffMs = { 0 },
            maxAttempts = 2,
            silenceTimeoutMs = 0,
        )
        val thread = Thread({ attachment.run() }, "trickle-and-drop")
        thread.isDaemon = true
        thread.start()

        val want = screenOf(uninterrupted(bytes))
        waitUntil(90_000, "the second, complete replay to finish") { screenOf(terminal) == want }
        attachment.stop()
        thread.join(5_000)

        assertEquals(2, attachment.attempts, "the connection never dropped, so this proved nothing")
        assertEquals(want, screenOf(terminal))
        assertNotEquals(0, machine.dataFramesSent.get())
    }

    @Test
    @Timeout(60)
    fun `the drop this file relies on really is a drop`() {
        // The guard the other tests need. If `dropAfterBytes` stopped working,
        // every reconnect assertion above would pass by never reconnecting.
        val bytes = fixture()
        val machine = FakeMachine(
            scrollback = bytes,
            chunkBytes = 64,
            dropAfterBytes = 200,
            dropOnAttempts = setOf(1, 2, 3),
        )
        val events = ConcurrentLinkedQueue<AttachmentEvent>()
        val attachment = PtyAttachment(
            connect = { machine.open() },
            attachRequest = { """{"cmd":"attach","id":1,"cols":80,"rows":24}""" },
            terminal = Terminal(80, 24),
            onEvent = { events.add(it) },
            backoffMs = { 0 },
            maxAttempts = 3,
            silenceTimeoutMs = 0,
        )
        val thread = Thread({ attachment.run() }, "always-drops")
        thread.isDaemon = true
        thread.start()
        thread.join(TimeUnit.SECONDS.toMillis(30))
        attachment.stop()

        assertEquals(3, attachment.attempts)
        assertTrue(
            events.count { it is AttachmentEvent.Lost } >= 2,
            "a machine set to drop every time reported no losses: ${events.toList()}",
        )
    }
}
