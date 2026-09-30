package com.rimeos.remote.core.agent

import com.rimeos.remote.core.link.AttachmentEvent
import com.rimeos.remote.core.link.Disconnected
import com.rimeos.remote.core.link.FakeMachine
import com.rimeos.remote.core.link.PtyAttachment
import com.rimeos.remote.core.term.Terminal
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.Timeout
import org.junit.jupiter.api.assertThrows

/**
 * The control link as something the phone holds open: the watchdog, one
 * handshake per connection, terminals on the same connection, and a reply
 * that submits.
 */
class StayConnectedTest {
    private val ok = """{"reply":"ok"}"""

    private fun waitUntil(ms: Long, what: () -> Boolean) {
        val deadline = System.nanoTime() + ms * 1_000_000
        while (!what() && System.nanoTime() < deadline) Thread.sleep(10)
    }

    // ---- the watchdog --------------------------------------------------------

    @Test
    @Timeout(20)
    fun `a silent connection fails its waiting request in seconds, not five minutes`() {
        // The machine takes the request and then says nothing at all — no
        // reply, no ping. That is a phone that walked out of Wi-Fi range: no
        // FIN, no error, and before the watchdog, a five-minute wait.
        val machine = FakeMachine(control = { Thread.sleep(10_000); ok }, pingEveryMs = 0)
        val events = CopyOnWriteArrayList<MachineLink.LinkEvent>()
        val link = MachineLink({ machine.open() }, { events.add(it) }, silenceMs = 300, watchdogPollMs = 50)
        val started = System.nanoTime()
        assertThrows<Disconnected> { link.request(Agentd.list(), retry = false) }
        val ms = (System.nanoTime() - started) / 1_000_000
        assertTrue(ms < 5_000, "the dead connection held the request for ${ms}ms")
        assertTrue(events.any { it is MachineLink.LinkEvent.Silent }, "no Silent before the drop: $events")
        assertTrue(events.any { it is MachineLink.LinkEvent.Dropped }, "$events")
        assertFalse(link.connected)
        link.close()
    }

    @Test
    @Timeout(20)
    fun `a slow request on a connection that is still pinging is left alone`() {
        // The anti-vacuity half: the same slow answer, with the desktop's
        // keepalive flowing. A watchdog that fired here would reconnect every
        // time the machine took a moment to think.
        val machine = FakeMachine(
            control = { Thread.sleep(1_000); """{"reply":"sessions","sessions":[]}""" },
            pingEveryMs = 50,
        )
        val events = CopyOnWriteArrayList<MachineLink.LinkEvent>()
        val link = MachineLink({ machine.open() }, { events.add(it) }, silenceMs = 300, watchdogPollMs = 50)
        assertEquals(emptyList<AgentSession>(), link.sessions())
        assertFalse(events.any { it is MachineLink.LinkEvent.Silent }, "$events")
        assertEquals(1, machine.connections.get())
        link.close()
    }

    // ---- one handshake --------------------------------------------------------

    @Test
    @Timeout(20)
    fun `callers who all find the link down make one connection between them`() {
        val machine = FakeMachine(control = { """{"reply":"sessions","sessions":[]}""" })
        val link = MachineLink({ Thread.sleep(100); machine.open() })
        val go = CountDownLatch(1)
        val threads = (1..6).map {
            Thread {
                go.await()
                link.sessions()
            }.also { it.start() }
        }
        go.countDown()
        threads.forEach { it.join(10_000) }
        assertEquals(1, machine.connections.get(), "concurrent callers each made a handshake")
        assertEquals(1, link.connections)
        link.close()
    }

    @Test
    @Timeout(20)
    fun `connectNow opens the connection without asking anything, once`() {
        val machine = FakeMachine()
        val link = MachineLink({ machine.open() })
        link.connectNow()
        link.connectNow()
        assertTrue(link.connected)
        assertEquals(1, machine.connections.get())
        assertTrue(machine.requests.isEmpty(), "connecting asked the machine something")
        link.disconnect()
        assertFalse(link.connected)
        link.close()
    }

    @Test
    @Timeout(20)
    fun `an old rime-remoted's refusal of remote_hello is read as no features`() {
        val machine = FakeMachine(
            control = {
                """{"reply":"error","kind":"bad_request","message":"unparseable request: unknown variant `remote_hello`"}"""
            },
        )
        MachineLink({ machine.open() }).use { link ->
            val r = link.remoteHello()
            assertTrue(r.features.isEmpty())
            assertTrue(r.lan.isEmpty())
        }
    }

    // ---- replies go to the request that asked -----------------------------------

    /**
     * A channel that holds the FIRST control frame back until a second one
     * has been written (or 300 ms pass), then writes it — which is exactly
     * the interleaving two threads produce when queueing a reply slot and
     * writing the request are separate steps.
     */
    private class Reordering(private val inner: com.rimeos.remote.core.FrameChannel) :
        com.rimeos.remote.core.FrameChannel by inner {
        val firstParked = CountDownLatch(1)
        private val secondSent = CountDownLatch(1)
        private val first = java.util.concurrent.atomic.AtomicBoolean(true)

        override fun send(frame: com.rimeos.remote.core.Frame) {
            if (frame is com.rimeos.remote.core.Frame.Control && first.compareAndSet(true, false)) {
                firstParked.countDown()
                secondSent.await(300, TimeUnit.MILLISECONDS)
                inner.send(frame)
                return
            }
            inner.send(frame)
            if (frame is com.rimeos.remote.core.Frame.Control) secondSent.countDown()
        }
    }

    @Test
    @Timeout(20)
    fun `two questions asked at once each get their own answer`() {
        // The machine echoes the line it was sent, so a swapped reply is
        // visible as one request receiving the other's words. Every reply in
        // the other tests here is identical, which is why none of them could
        // see this: the hub now asks `hello` and `remote_hello` together, and
        // a swap made every feature read as absent.
        val machine = FakeMachine(control = { it })
        var wrapped: Reordering? = null
        val link = MachineLink({ Reordering(machine.open()).also { wrapped = it } })
        link.connectNow()
        val a = """{"cmd":"info","id":1}"""
        val b = """{"cmd":"info","id":2}"""
        var gotA: String? = null
        var gotB: String? = null
        val ta = Thread { gotA = link.request(a) }.apply { start() }
        assertTrue(wrapped!!.firstParked.await(5, TimeUnit.SECONDS))
        val tb = Thread { gotB = link.request(b) }.apply { start() }
        ta.join(10_000)
        tb.join(10_000)
        assertEquals(a, gotA, "the first request was handed the second one's reply")
        assertEquals(b, gotB, "the second request was handed the first one's reply")
        assertEquals(listOf(a, b), machine.requests.toList(), "requests reached the wire out of order")
        link.close()
    }

    // ---- a terminal on the control connection ---------------------------------

    @Test
    @Timeout(30)
    fun `with mux_attach a terminal rides the control connection and leaves it up`() {
        val machine = FakeMachine(
            scrollback = "hello from the machine\r\n".toByteArray(),
            control = { line ->
                if (line.contains("\"list\"")) """{"reply":"sessions","sessions":[]}""" else ok
            },
        )
        val link = MachineLink({ machine.open() })
        link.connectNow()
        val attached = CountDownLatch(1)
        val events = CopyOnWriteArrayList<AttachmentEvent>()
        val attachment = PtyAttachment(
            connect = { error("a shared terminal dialled a connection of its own") },
            attachRequest = { Agentd.attach(3, 80, 24) },
            terminal = Terminal(80, 24),
            onEvent = {
                events.add(it)
                if (it is AttachmentEvent.Attached) attached.countDown()
            },
            backoffMs = { 0L },
            shared = { link.shared() },
        )
        val runner = Thread({ attachment.run() }, "shared-attachment").apply { isDaemon = true; start() }
        assertTrue(attached.await(10, TimeUnit.SECONDS), "never attached: $events")
        waitUntil(5_000) { attachment.terminal.read { it.row(0).text() }.startsWith("hello") }
        assertTrue(attachment.terminal.read { it.row(0).text() }.startsWith("hello from the machine"))

        // Control still works on the same connection while the terminal is up.
        assertEquals(emptyList<AgentSession>(), link.sessions())
        assertTrue(attachment.send("x".toByteArray()))
        waitUntil(5_000) { machine.receivedBytes().isNotEmpty() }
        assertEquals("x", String(machine.receivedBytes()))

        // Leaving the terminal closes only its channel.
        attachment.stop()
        runner.join(5_000)
        assertFalse(runner.isAlive, "the attachment did not stop")
        assertTrue(link.connected, "closing a terminal took the control connection with it")
        assertEquals(emptyList<AgentSession>(), link.sessions())
        assertEquals(1, machine.connections.get(), "a terminal and control used two connections")
        link.close()
    }

    @Test
    @Timeout(30)
    fun `a terminal on a shared connection re-attaches when that connection drops`() {
        val machine = FakeMachine(scrollback = "again\r\n".toByteArray())
        val link = MachineLink({ machine.open() })
        val attached = CountDownLatch(2)
        val attachment = PtyAttachment(
            connect = { error("not used") },
            attachRequest = { Agentd.attach(3, 80, 24) },
            terminal = Terminal(80, 24),
            onEvent = { if (it is AttachmentEvent.Attached) attached.countDown() },
            backoffMs = { 0L },
            shared = { link.shared() },
        )
        val runner = Thread({ attachment.run() }, "shared-reattach").apply { isDaemon = true; start() }
        waitUntil(5_000) { attachment.attached }
        machine.hangUp()
        assertTrue(attached.await(10, TimeUnit.SECONDS), "the terminal did not come back")
        assertEquals(2, machine.connections.get())
        attachment.stop()
        runner.join(5_000)
        link.close()
    }

    // ---- a reply that submits -------------------------------------------------

    @Test
    @Timeout(20)
    fun `against an older daemon the Return is its own request, a moment after the text`() {
        val machine = FakeMachine(control = { ok })
        val slept = ArrayList<Long>()
        MachineLink({ machine.open() }).use { link ->
            link.reply(3, "a long answer", submitSupported = false, sleep = { slept.add(it) })
        }
        assertEquals(
            listOf(
                """{"cmd":"input","id":3,"data":"a long answer"}""",
                """{"cmd":"input","id":3,"data":"\r"}""",
            ),
            machine.requests.toList(),
        )
        assertEquals(listOf(100L), slept)
    }

    @Test
    @Timeout(20)
    fun `against a daemon with input_submit it is one request`() {
        val machine = FakeMachine(control = { ok })
        MachineLink({ machine.open() }).use { link ->
            link.reply(3, "yes please", submitSupported = true, sleep = { error("slept") })
        }
        assertEquals(
            listOf("""{"cmd":"input","id":3,"data":"yes please","submit":true}"""),
            machine.requests.toList(),
        )
    }

    @Test
    @Timeout(20)
    fun `typed but not submitted is reported as that, and nothing is typed twice`() {
        val machine = FakeMachine(
            control = { line ->
                if (line.contains(""""data":"\r"""")) {
                    """{"reply":"error","kind":"session_exited","message":"session 3 has exited"}"""
                } else {
                    ok
                }
            },
        )
        MachineLink({ machine.open() }).use { link ->
            val e = assertThrows<NotSubmitted> { link.reply(3, "text", submitSupported = false, sleep = {}) }
            assertTrue(e.cause is AgentError)
        }
        assertEquals(2, machine.requests.size, "the text was sent again")

        // A refusal of the FIRST request is not "typed": nothing is on the line.
        val refusing = FakeMachine(
            control = { """{"reply":"error","kind":"no_such_session","message":"no session 3"}""" },
        )
        MachineLink({ refusing.open() }).use { link ->
            assertThrows<AgentError> { link.reply(3, "text", submitSupported = false, sleep = {}) }
        }
        assertEquals(1, refusing.requests.size)
    }

    @Test
    @Timeout(20)
    fun `rename and peek travel as the contract's requests`() {
        val tail = java.util.Base64.getEncoder().encodeToString("out\r\n".toByteArray())
        val machine = FakeMachine(
            control = { line ->
                when {
                    line.contains("\"rename\"") ->
                        """{"reply":"session","id":3,"agent":"claude","name":"auth refactor","title":null}"""
                    line.contains("\"peek\"") ->
                        """{"reply":"peek","id":3,"data":"$tail","cols":80,"rows":24,"state":"working"}"""
                    else -> ok
                }
            },
        )
        MachineLink({ machine.open() }).use { link ->
            assertEquals("auth refactor", link.rename(3, "auth refactor").name)
            assertEquals(listOf("out"), LiveTail.lines(link.peek(3)))
        }
        assertEquals(
            listOf(
                """{"cmd":"rename","id":3,"name":"auth refactor"}""",
                """{"cmd":"peek","id":3,"bytes":8192}""",
            ),
            machine.requests.toList(),
        )
    }
}
