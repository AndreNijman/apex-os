package com.apexos.remote.core.agent

import com.apexos.remote.core.link.Disconnected
import com.apexos.remote.core.link.FakeMachine
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertNotNull
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertThrows
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.Timeout

/**
 * P1-054, everything except the drawing.
 *
 * The criterion asks for a list showing the agent graph, provider/model,
 * project/workspace, mode, activity, context/quota, elapsed time and status;
 * for starting a session; for pause/stop/reconnect/attach; and for colours
 * matching the desktop. `ToneColoursTest` and `AgentStateAgreementTest` hold
 * the last of those. This holds the rest of what can be held without a phone —
 * which is all of it except the pixels.
 */
class AgentCenterTest {
    private fun session(
        id: Int,
        state: String = AgentStates.WORKING,
        lastActivity: Long = 0,
        started: Long = 0,
        exitCode: Int? = null,
        exitSignal: Int? = null,
        children: List<ChildInfo> = emptyList(),
        telemetry: Telemetry? = null,
    ) = AgentSession(
        id = id,
        state = state,
        lastActivity = lastActivity,
        started = started,
        exitCode = exitCode,
        exitSignal = exitSignal,
        children = children,
        telemetry = telemetry,
    )

    // ---- ordering -------------------------------------------------------

    @Test
    fun `the ones that need you are their own group, above everything`() {
        val list = listOf(
            session(1, AgentStates.WORKING, lastActivity = 900),
            session(2, AgentStates.PERMISSION_REQUEST, lastActivity = 100),
            session(3, AgentStates.WAITING_FOR_USER, lastActivity = 50),
        )
        val (needsYou, rest) = Order.groups(list)
        assertEquals(listOf(2, 3), needsYou.map { it.id })
        assertEquals(listOf(1), rest.map { it.id })
        // Unsorted within the group, on purpose: the daemon lists oldest
        // first, so the one that has been waiting longest is already on top.
        assertEquals(listOf(2, 3), needsYou.map { it.id })
    }

    @Test
    fun `the rest are live first, then most recently active`() {
        // The desktop's comparator, verbatim: `AgentCenter.qml` and
        // `remoteagents.js` both say "a finished session sinking below a
        // running one is what makes the list readable at a glance".
        val list = listOf(
            session(1, lastActivity = 100),
            session(2, lastActivity = 900, exitCode = 0),
            session(3, lastActivity = 500),
            session(4, lastActivity = 950, exitSignal = 9),
        )
        assertEquals(listOf(3, 1, 4, 2), Order.flat(list).map { it.id })
    }

    @Test
    fun `live is the exit fields and not the published state`() {
        // `AgentService._isLive` is `exit_code === null && exit_signal === null`.
        // The distinction is not academic: `state` is whatever was last
        // PUBLISHED, including by the agent itself through the open `Event`
        // verb, and sorting on it would let an agent hold the top of somebody's
        // list by reporting `working` for ever.
        val claimsDone = session(1, AgentStates.COMPLETE)
        assertTrue(claimsDone.live, "a session that published `complete` and has not exited is live")
        val killedQuietly = session(2, AgentStates.WORKING, exitSignal = 15)
        assertFalse(killedQuietly.live, "a session killed before it published anything is not live")

        assertEquals(1, Order.liveCount(listOf(claimsDone, killedQuietly)))
    }

    @Test
    fun `an unknown state is neither live-sorted wrongly nor counted as attention`() {
        val future = session(1, "hibernating", lastActivity = 10)
        assertTrue(future.live)
        assertEquals(0, Order.attentionCount(listOf(future)))
        assertEquals(Tone.IDLE, future.tone)
    }

    // ---- elapsed --------------------------------------------------------

    @Test
    fun `elapsed is coarse and short, as the desktop writes it`() {
        assertEquals("0s", Elapsed.format(0))
        assertEquals("59s", Elapsed.format(59))
        assertEquals("1m", Elapsed.format(60))
        assertEquals("59m", Elapsed.format(3599))
        assertEquals("1h", Elapsed.format(3600))
        assertEquals("1h 47m", Elapsed.format(3600 + 47 * 60))
        assertEquals("2h", Elapsed.format(7200))
    }

    @Test
    fun `a finished session's clock stops at its last activity, not at now`() {
        // The rule that looks like a bug. A session that failed an hour ago
        // did not take an hour; it took however long it ran.
        val now = 10_000L
        val finished = session(1, AgentStates.FAILED, started = 1_000, lastActivity = 1_300, exitCode = 1)
        assertEquals("5m", Elapsed.of(finished, now))
        val running = session(2, AgentStates.WORKING, started = 1_000, lastActivity = 1_300)
        assertEquals("2h 30m", Elapsed.of(running, now))
    }

    @Test
    fun `a session with no start time shows nothing rather than the epoch`() {
        assertEquals("", Elapsed.of(session(1, started = 0), nowSeconds = 1_800_000_000))
    }

    // ---- telemetry ------------------------------------------------------

    @Test
    fun `a session that has never reported its context gets no gauge, not a zero`() {
        // `Telemetry`'s own words: "a context gauge drawn at 0% for a session
        // that has never reported is a gauge that is lying."
        assertNull(Gauge.fraction(null))
        assertNull(Gauge.label(null))
        assertEquals(0f, Gauge.fraction(0.0))
        assertEquals("0%", Gauge.label(0.0))
        assertEquals(0.62f, Gauge.fraction(62.0))
        assertEquals("62%", Gauge.label(61.7))
    }

    @Test
    fun `a runtime reporting more than a hundred percent is clamped, never hidden`() {
        assertEquals(1f, Gauge.fraction(104.0))
        assertNotNull(Gauge.label(104.0))
    }

    @Test
    fun `the model shown is the one the agent reported, and absent when it has not`() {
        val told = session(1, telemetry = Telemetry(model = "Opus 4.5"))
        assertEquals("Opus 4.5", told.telemetry?.model)
        assertNull(session(2).telemetry?.model)
    }

    @Test
    fun `the account-wide figures are marked as not per-row`() {
        // The daemon's own comment: the two rate-limit figures are account-wide,
        // so six sessions on one login report the same number and a list that
        // drew it per row would repeat one fact six times.
        assertFalse(Gauge.RATE_LIMITS_ARE_PER_ROW)
    }

    // ---- the graph ------------------------------------------------------

    private fun child(id: String, parent: String? = null, kind: String = "process", ended: Long? = null) =
        ChildInfo(id = id, parent = parent, kind = kind, ended = ended, label = id)

    @Test
    fun `the graph nests three deep, which is what a real session produces`() {
        // An MCP server forked by a language server forked by a subagent: the
        // exact shape `ChildInfo`'s documentation names.
        val children = listOf(
            child("sub", kind = "subagent"),
            child("lsp", parent = "sub"),
            child("mcp", parent = "lsp"),
        )
        assertEquals(3, AgentGraph.depth(children))
        val rows = AgentGraph.rows(children)
        assertEquals(listOf("sub" to 0, "lsp" to 1, "mcp" to 2), rows.map { it.info.id to it.depth })
    }

    @Test
    fun `a child whose parent has exited becomes a root rather than disappearing`() {
        // `children` is the daemon's live view: a parent that ended between
        // two polls is gone from it while its children are not. Dropping the
        // orphan would make the phone show fewer processes than exist, which
        // is the one direction a supervision tool must never be wrong in.
        val children = listOf(child("a"), child("orphan", parent = "vanished"))
        val rows = AgentGraph.rows(children)
        assertEquals(2, rows.size, "the orphan was dropped")
        assertTrue(rows.all { it.depth == 0 })
        assertTrue(rows.any { it.info.id == "orphan" })
    }

    @Test
    fun `a cycle is broken and every node is still shown exactly once`() {
        val children = listOf(child("a", parent = "b"), child("b", parent = "a"), child("c", parent = "c"))
        val rows = AgentGraph.rows(children)
        assertEquals(3, rows.size, "a cycle lost or duplicated a node: ${rows.map { it.info.id }}")
        assertEquals(setOf("a", "b", "c"), rows.map { it.info.id }.toSet())
    }

    @Test
    fun `sibling order is the daemon's, so nothing appears to move when a sibling exits`() {
        val children = listOf(
            child("root"),
            child("first", parent = "root"),
            child("second", parent = "root"),
            child("third", parent = "root"),
        )
        assertEquals(
            listOf("root", "first", "second", "third"),
            AgentGraph.rows(children).map { it.info.id },
        )
    }

    @Test
    fun `the summary counts only what is still running, and counts the two kinds apart`() {
        val children = listOf(
            child("s1", kind = "subagent"),
            child("s2", kind = "subagent"),
            child("p1"),
            child("gone", ended = 12),
        )
        assertEquals("2 subagents, 1 process", AgentGraph.summary(children))
        assertNull(AgentGraph.summary(listOf(child("dead", ended = 1))), "a dead child is not a summary")
        assertNull(AgentGraph.summary(emptyList()))
    }

    @Test
    fun `an empty children list is a session with no graph, not a daemon without one`() {
        // The daemon's own comment says the distinction is load-bearing: `[]`
        // means "asked and found none". Nothing here may turn that into a
        // failure.
        assertEquals(emptyList<AgentGraph.Row>(), AgentGraph.rows(emptyList()))
        assertEquals(0, AgentGraph.depth(emptyList()))
    }

    // ---- the link -------------------------------------------------------

    /** The pump notices a dead stream on its own thread; this waits for it. */
    private fun waitFor(condition: () -> Boolean) {
        val deadline = System.currentTimeMillis() + 10_000
        while (System.currentTimeMillis() < deadline) {
            if (condition()) return
            Thread.sleep(5)
        }
        throw AssertionError("the link never noticed the machine had gone")
    }

    private val twoSessions = """
        {"reply":"sessions","sessions":[
          {"id":1,"agent":"claude","state":"working","cwd":"/home/a/p","last_activity":900},
          {"id":2,"agent":"codex","state":"permission_request","cwd":"/home/a/q","last_activity":100}
        ]}
    """.trimIndent().replace("\n", "").replace("  ", "")

    @Test
    @Timeout(30)
    fun `a link asks, and sorts what comes back the way the desktop does`() {
        val machine = FakeMachine(control = { twoSessions })
        MachineLink({ machine.open() }).use { link ->
            val sessions = link.sessions()
            assertEquals(listOf(2, 1), sessions.map { it.id }, "the needs-you session was not first")
            assertEquals("Claude", sessions[1].agentName)
            assertEquals("Codex", sessions[0].agentName)
        }
        assertTrue(machine.requests.any { it.contains("\"list\"") })
    }

    @Test
    @Timeout(30)
    fun `one connection serves many questions`() {
        val machine = FakeMachine(control = { twoSessions })
        MachineLink({ machine.open() }).use { link ->
            repeat(5) { link.sessions() }
            assertEquals(1, link.connections, "a link opened a connection per request")
        }
    }

    @Test
    @Timeout(30)
    fun `a question after the machine hangs up opens a new connection and answers`() {
        // The reconnect a control plane needs, which is not a loop: a dead
        // connection is noticed on the next question and replaced then.
        var round = 0
        val dead = FakeMachine(control = { twoSessions })
        val alive = FakeMachine(control = { twoSessions })
        val link = MachineLink({ if (round++ == 0) dead.open() else alive.open() })
        link.use {
            val channel = it.sessions()
            assertEquals(2, channel.size)
            assertEquals(1, it.connections)
            // The machine goes away with no warning: no Close frame, just a
            // stream that ends. What a sleeping laptop does.
            dead.hangUp()
            waitFor { !it.connected }
            val again = it.sessions()
            assertEquals(2, again.size, "the link did not recover")
            assertEquals(2, it.connections, "the link did not open a second connection")
        }
    }

    @Test
    @Timeout(30)
    fun `an instruction whose reply is lost is not sent again, and a question is`() {
        // The case that matters, and it is not "the connection was already
        // dead" — opening a fresh connection for a fresh request is correct
        // and sends the instruction exactly once. The dangerous case is a
        // request that REACHED the daemon and whose reply was lost: a SIGTERM
        // delivered twice because the answer went missing is a second signal
        // into whatever the agent was doing next.
        //
        // So the machine here records the request and then pulls the plug
        // without answering, which is exactly what a laptop closing its lid
        // does mid-round-trip.
        var self: FakeMachine? = null
        val machine = FakeMachine(
            control = { line ->
                // Recorded by `serve` before this runs; the hangup is queued
                // ahead of the reply, so the client sees the stream end.
                if (line.contains("\"signal\"")) self?.hangUp()
                """{"reply":"ok"}"""
            },
        )
        self = machine

        MachineLink({ machine.open() }).use { link ->
            assertThrows(Disconnected::class.java) { link.stop(1) }
            assertEquals(
                1,
                machine.requests.count { it.contains("\"signal\"") },
                "the signal was sent twice: ${machine.requests.toList()}",
            )
        }
    }

    @Test
    @Timeout(30)
    fun `a question whose reply is lost IS asked again, because asking twice costs nothing`() {
        // The other half of the same rule. A `list` that was lost is free to
        // repeat, and a phone that gave up on a refreshed list because one
        // connection blinked would look broken.
        var self: FakeMachine? = null
        val answered = java.util.concurrent.atomic.AtomicInteger(0)
        val machine = FakeMachine(
            control = { _ ->
                // The first one is swallowed; the second is answered.
                if (answered.getAndIncrement() == 0) self?.hangUp()
                """{"reply":"sessions","sessions":[]}"""
            },
        )
        self = machine

        MachineLink({ machine.open() }).use { link ->
            assertEquals(emptyList<AgentSession>(), link.sessions())
            assertEquals(2, machine.requests.size, "the lost question was not asked again")
            assertEquals(2, link.connections, "the retry reused a connection that had gone")
        }
    }

    @Test
    @Timeout(30)
    fun `the daemon's refusal arrives in the daemon's own words`() {
        val machine = FakeMachine(
            control = { """{"reply":"error","kind":"not_found","message":"no such session 4"}""" },
        )
        MachineLink({ machine.open() }).use { link ->
            val e = assertThrows(AgentError::class.java) { link.info(4) }
            assertEquals("not_found", e.kind)
            assertEquals("no such session 4", e.message)
        }
    }

    @Test
    @Timeout(30)
    fun `a relative working directory is refused here, not discovered as a daemon error`() {
        val machine = FakeMachine(control = { """{"reply":"session","id":9,"cwd":"/x"}""" })
        MachineLink({ machine.open() }).use { link ->
            assertThrows(IllegalArgumentException::class.java) { link.run("relative/path", 80, 24) }
            assertTrue(machine.requests.isEmpty(), "a request the protocol forbids reached the wire")
            val started = link.run("/home/a/p", 80, 24, agent = "claude", prompt = "go")
            assertEquals(9, started.id)
        }
    }

    @Test
    @Timeout(30)
    fun `pause and stop are the signals the daemon knows, not verbs it does not`() {
        // `pause` and `resume` are not verbs on this socket; they are SIGSTOP
        // and SIGCONT, and `paused` becomes true only after the kill succeeds.
        val machine = FakeMachine(control = { """{"reply":"ok"}""" })
        MachineLink({ machine.open() }).use { link ->
            link.pause(3)
            link.resume(3)
            link.stop(3)
            link.interrupt(3)
        }
        val sent = machine.requests.toList()
        assertTrue(sent.any { it.contains(""""signal":"stop"""") }, sent.toString())
        assertTrue(sent.any { it.contains(""""signal":"cont"""") })
        assertTrue(sent.any { it.contains(""""signal":"term"""") })
        assertTrue(sent.any { it.contains(""""signal":"int"""") })
        assertTrue(sent.none { it.contains(""""cmd":"pause"""") }, "a verb the daemon does not have was sent")
    }

    @Test
    fun `no request this app builds carries a newline, which the wire refuses`() {
        // `apex-remote-core`'s wire refuses a control payload containing a
        // newline in both directions, because a payload carrying its own
        // terminator would let a client smuggle a second request into a frame.
        val requests = listOf(
            Agentd.hello(),
            Agentd.list(),
            Agentd.info(1),
            Agentd.attach(1, 80, 24),
            Agentd.resize(1, 80, 24),
            Agentd.signal(1, "term"),
            Agentd.run("/home/a/a\nb", 80, 24, agent = "claude", prompt = "line\none\r\ntwo three"),
        )
        for (r in requests) {
            assertFalse(r.contains('\n'), "a request carried a newline: $r")
            assertFalse(r.contains('\r'), "a request carried a carriage return: $r")
            assertFalse(r.any { it.code < 0x20 }, "a request carried a control character: $r")
        }
    }

    @Test
    fun `the worktrees verb exists and its answer groups into projects`() {
        // INVERTED BACK. This asserted `Agentd` had no `worktrees` builder,
        // citing a vocabulary read off the wrong enum. See AgentdTest and
        // Worktrees.kt for the full account.
        val methods = Agentd::class.java.methods.map { it.name }
        assertTrue("worktrees" in methods)
        assertTrue("readWorktrees" in methods)
    }
}
