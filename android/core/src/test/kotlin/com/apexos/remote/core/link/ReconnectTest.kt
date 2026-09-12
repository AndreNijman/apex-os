package com.apexos.remote.core.link

import com.apexos.remote.core.term.Terminal
import com.apexos.remote.core.term.TuiFixtures
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertNotEquals
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.Timeout
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * P1-055: "reconnect", and "low-bandwidth and reconnect behaviour is tested".
 *
 * ## The trap this file is written against
 *
 * A reconnect test that passes because nothing ever disconnected has proved
 * nothing at all. So every test here asserts **that the connection actually
 * died** — by counting the connections the machine handed out — before it
 * asserts anything about recovery. [aReconnectTestThatCannotPassWithoutADrop]
 * is the guard on that guard: it runs the identical scenario with the drop
 * turned off and requires the connection count to differ.
 *
 * ## Why the screen must come back identical and not merely "look fine"
 *
 * `apex-agentd` replays its scrollback to every attaching client
 * (`registry.rs`: `Session::attach` writes `scrollback.tail(replay)` before
 * the client is added to the attachers). So a phone that reconnects is sent
 * the history **again**. A terminal that did not reset first would show the
 * last few hundred lines twice, and nothing about that looks like an error —
 * it looks like the agent repeated itself.
 *
 * The assertion is therefore equality with a terminal that was never
 * interrupted, cell for cell.
 *
 * ## The deadlines are not boilerplate
 *
 * Every test here has threads waiting on queues. A bug that leaves the
 * attachment loop waiting for a frame that will never come does not fail the
 * suite without a deadline: it hangs it.
 */
class ReconnectTest {
    private fun fixture(): ByteArray = TuiFixtures.read(TuiFixtures.CLAUDE_SESSION)

    /** A terminal fed the stream once, with nothing going wrong. */
    private fun uninterrupted(bytes: ByteArray): Terminal {
        val t = Terminal(80, 24)
        t.feed(bytes)
        return t
    }

    private fun screenOf(t: Terminal): String = buildString {
        val s = t.screen
        for (i in 0 until s.totalLines) {
            val line = s.lineAt(i) ?: continue
            for (c in 0 until line.cols) {
                append(line.code[c]).append(':').append(line.fg[c]).append(':')
                    .append(line.bg[c]).append(':').append(line.attrs[c]).append(' ')
            }
            append('\n')
        }
        append("cursor=").append(t.cursorRow).append(',').append(t.cursorCol)
    }

    @Test
    @Timeout(30)
    fun `a connection that dies mid-stream comes back and the screen is not doubled`() {
        val stream = fixture()
        val machine = FakeMachine(
            name = "l16",
            scrollback = stream,
            // The plug is pulled part-way through the replay, which on this
            // fixture is in the middle of a truecolor SGR sequence.
            dropAfterBytes = 900,
            dropOnAttempts = setOf(1),
        )
        val attached = CountDownLatch(2)
        val events = CopyOnWriteArrayList<AttachmentEvent>()
        val attachment = PtyAttachment(
            connect = { machine.open() },
            attachRequest = { """{"cmd":"attach","id":4,"cols":80,"rows":24,"replay":262144}""" },
            terminal = Terminal(80, 24),
            onEvent = {
                events.add(it)
                if (it is AttachmentEvent.Attached) attached.countDown()
            },
            backoffMs = { 0L },
            maxAttempts = 2,
        )
        val runner = Thread({ attachment.run() }, "attachment")
        runner.isDaemon = true
        runner.start()

        // Two attachments: the one that died and the one that replaced it.
        assertTrue(attached.await(20, TimeUnit.SECONDS), "the second attach never happened; events=$events")
        // The second attempt receives the whole stream; wait for it to land.
        waitUntil(20_000) { screenOf(attachment.terminal) == screenOf(uninterrupted(stream)) }
        attachment.stop()
        runner.join(5_000)

        assertEquals(2, machine.connections.get(), "the connection did not actually drop and reopen")
        assertEquals(2, attachment.attachments, "two attaches were expected; events=$events")
        assertEquals(
            screenOf(uninterrupted(stream)),
            screenOf(attachment.terminal),
            "the recovered screen differs from the one that was never interrupted",
        )
    }

    @Test
    @Timeout(30)
    fun aReconnectTestThatCannotPassWithoutADrop() {
        // The anti-vacuity guard. The identical scenario with the plug left
        // alone must take ONE connection, so the test above is measuring the
        // drop and not the passage of time.
        val stream = fixture()
        val machine = FakeMachine(name = "l16", scrollback = stream)
        val attached = CountDownLatch(1)
        val attachment = PtyAttachment(
            connect = { machine.open() },
            attachRequest = { """{"cmd":"attach","id":4,"cols":80,"rows":24,"replay":262144}""" },
            terminal = Terminal(80, 24),
            onEvent = { if (it is AttachmentEvent.Attached) attached.countDown() },
            backoffMs = { 0L },
            maxAttempts = 2,
        )
        val runner = Thread({ attachment.run() }, "attachment-nodrop")
        runner.isDaemon = true
        runner.start()
        assertTrue(attached.await(20, TimeUnit.SECONDS))
        waitUntil(10_000) { screenOf(attachment.terminal) == screenOf(uninterrupted(stream)) }
        assertEquals(1, machine.connections.get(), "nothing dropped, so there must be exactly one connection")
        assertEquals(1, attachment.attachments)
        attachment.stop()
        runner.join(5_000)
    }

    @Test
    @Timeout(30)
    fun `a refused attach stops rather than retrying forever`() {
        // "No such session 4" does not get better by being asked again, and a
        // phone that retried it every second would be a phone with a dead
        // battery and a machine with a log full of refusals.
        val machine = FakeMachine(
            name = "l16",
            attachReply = { """{"reply":"error","kind":"no_such_session","message":"no session 4"}""" },
        )
        val events = CopyOnWriteArrayList<AttachmentEvent>()
        val attachment = PtyAttachment(
            connect = { machine.open() },
            attachRequest = { """{"cmd":"attach","id":4,"cols":80,"rows":24}""" },
            terminal = Terminal(80, 24),
            onEvent = { events.add(it) },
            backoffMs = { 0L },
        )
        attachment.run() // returns, rather than looping
        assertEquals(1, machine.connections.get(), "a refusal was retried")
        val refused = events.filterIsInstance<AttachmentEvent.Refused>()
        assertEquals(1, refused.size, "events=$events")
        assertTrue(refused[0].reply.contains("no session 4"), "the machine's own words must reach the screen")
    }

    @Test
    @Timeout(30)
    fun `an open answered with only a close is reported and does not desynchronise the queue`() {
        // The ordering trap. `apex-remoted` sends NO control reply when the
        // proxy itself failed — only `Close(channel, reason)`. A queue that
        // waited for a control frame per open would hand the NEXT request's
        // reply to this attach, and every reply after it would be one behind.
        val machine = FakeMachine(
            name = "l16",
            closeWithoutReply = "the agent runtime is not reachable",
            control = { line ->
                when {
                    line.contains("\"list\"") -> """{"reply":"sessions","sessions":[]}"""
                    else -> """{"reply":"ok"}"""
                }
            },
        )
        val channel = machine.open()
        val mux = Mux(
            channel,
            object : Mux.Listener {
                override fun onData(channel: UInt, bytes: ByteArray) = Unit
                override fun onClose(channel: UInt, reason: String) = Unit
                override fun onDisconnect(cause: Throwable?) = Unit
            },
        )
        val pump = Thread({ mux.pump() }, "mux")
        pump.isDaemon = true
        pump.start()

        val refused = runCatching {
            mux.openChannel(1u, """{"cmd":"attach","id":4,"cols":80,"rows":24}""".toByteArray(), 10_000)
        }.exceptionOrNull()
        assertTrue(refused is ChannelRefused, "got $refused")
        assertTrue(refused!!.message!!.contains("not reachable"), refused.message!!)

        // And the very next request gets ITS OWN reply, which is the part a
        // desynchronised queue gets wrong.
        val reply = mux.request("""{"cmd":"list"}""", 10_000)
        assertEquals("""{"reply":"sessions","sessions":[]}""", reply)
        mux.close()
        pump.join(5_000)
    }

    @Test
    @Timeout(30)
    fun `replies are matched by order, because the wire carries no request id`() {
        val machine = FakeMachine(
            name = "l16",
            control = { line ->
                // Echo the request back inside the reply, so a mismatch is
                // visible rather than merely wrong.
                """{"reply":"echo","of":${line.substringAfter("\"n\":").substringBefore("}")}}"""
            },
        )
        val mux = Mux(
            machine.open(),
            object : Mux.Listener {
                override fun onData(channel: UInt, bytes: ByteArray) = Unit
                override fun onClose(channel: UInt, reason: String) = Unit
                override fun onDisconnect(cause: Throwable?) = Unit
            },
        )
        val pump = Thread({ mux.pump() }, "mux-order")
        pump.isDaemon = true
        pump.start()
        // Sequentially, which is what the FIFO guarantees for a single caller.
        for (n in 1..20) {
            assertEquals("""{"reply":"echo","of":$n}""", mux.request("""{"cmd":"x","n":$n}""", 10_000))
        }
        mux.close()
        pump.join(5_000)
    }

    @Test
    @Timeout(30)
    fun `an outstanding request fails when the connection dies rather than waiting out its deadline`() {
        // A caller blocked on a reply that can never come is the bug that
        // looks like a slow network for five minutes.
        val machine = FakeMachine(name = "l16", control = { throw IllegalStateException("never answers") })
        val channel = machine.open()
        val mux = Mux(
            channel,
            object : Mux.Listener {
                override fun onData(channel: UInt, bytes: ByteArray) = Unit
                override fun onClose(channel: UInt, reason: String) = Unit
                override fun onDisconnect(cause: Throwable?) = Unit
            },
        )
        val pump = Thread({ mux.pump() }, "mux-dies")
        pump.isDaemon = true
        pump.start()
        val asked = CountDownLatch(1)
        var failure: Throwable? = null
        val caller = Thread({
            failure = runCatching { mux.request("""{"cmd":"list"}""", 60_000) }.exceptionOrNull()
            asked.countDown()
        }, "caller")
        caller.isDaemon = true
        caller.start()
        Thread.sleep(200)
        channel.close()
        assertTrue(asked.await(10, TimeUnit.SECONDS), "the caller was still waiting after the connection died")
        assertTrue(failure is Disconnected, "got $failure")
        pump.join(5_000)
    }

    @Test
    @Timeout(30)
    fun `keystrokes typed while disconnected are dropped and say so`() {
        // The alternative is a queue that delivers a `y` into whatever the
        // agent happens to be doing thirty seconds later.
        val machine = FakeMachine(name = "l16", scrollback = "hello".toByteArray())
        val attached = CountDownLatch(1)
        val attachment = PtyAttachment(
            connect = { machine.open() },
            attachRequest = { """{"cmd":"attach","id":1,"cols":80,"rows":24}""" },
            terminal = Terminal(80, 24),
            onEvent = { if (it is AttachmentEvent.Attached) attached.countDown() },
            backoffMs = { 0L },
            maxAttempts = 1,
        )
        assertEquals(false, attachment.send("before".toByteArray()), "there was no connection to send on")
        val runner = Thread({ attachment.run() }, "attach-input")
        runner.isDaemon = true
        runner.start()
        assertTrue(attached.await(20, TimeUnit.SECONDS))
        assertTrue(attachment.send("ls\r".toByteArray()), "an attached terminal must accept input")
        waitUntil(10_000) { machine.receivedBytes().toString(Charsets.UTF_8).contains("ls\r") }
        val got = machine.receivedBytes().toString(Charsets.UTF_8)
        assertTrue(got.contains("ls\r"), "the machine received: $got")
        assertTrue(!got.contains("before"), "input typed before the connection existed was replayed: $got")
        attachment.stop()
        runner.join(5_000)
        assertEquals(false, attachment.send("after".toByteArray()), "a stopped attachment must not accept input")
    }

    @Test
    @Timeout(30)
    fun `the terminal's answers go back up the channel it was attached on`() {
        // A TUI's first act is to ask the terminal questions. If the answers
        // do not reach the PTY, the program waits forever and the screen is
        // blank — which is what the opencode recording shows happening.
        val machine = FakeMachine(name = "l16", scrollback = TuiFixtures.read(TuiFixtures.OPENCODE_QUERIES))
        val attached = CountDownLatch(1)
        val attachment = PtyAttachment(
            connect = { machine.open() },
            attachRequest = { """{"cmd":"attach","id":1,"cols":80,"rows":24}""" },
            terminal = Terminal(80, 24),
            onEvent = { if (it is AttachmentEvent.Attached) attached.countDown() },
            backoffMs = { 0L },
            maxAttempts = 1,
        )
        val runner = Thread({ attachment.run() }, "attach-answers")
        runner.isDaemon = true
        runner.start()
        assertTrue(attached.await(20, TimeUnit.SECONDS))
        waitUntil(10_000) { machine.receivedBytes().isNotEmpty() }
        val answers = machine.receivedBytes().toString(Charsets.ISO_8859_1)
        assertTrue(answers.contains("\u001b[1;1R"), "no cursor report reached the PTY: ${answers.take(200)}")
        assertTrue(answers.contains("\u001bP0+r4d73\u001b" + "\\"), "no terminfo answer reached the PTY")
        assertTrue(answers.contains("]11;rgb:"), "no background colour answer reached the PTY")
        assertNotEquals(0, answers.length)
        attachment.stop()
        runner.join(5_000)
    }

    private fun waitUntil(timeoutMs: Long, condition: () -> Boolean) {
        val deadline = System.currentTimeMillis() + timeoutMs
        while (System.currentTimeMillis() < deadline) {
            if (condition()) return
            Thread.sleep(20)
        }
    }
}
