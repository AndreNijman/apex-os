package com.apexos.remote.core.agent

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test


/**
 * The input-needed workflow: answering a waiting agent from a notification.
 *
 * The assertions that matter here are about the reply that must NOT be sent.
 * A reply delivered to the wrong session is not a display bug — it is an
 * instruction typed at an agent the user was not talking to.
 */
class ReplyTest {

    private fun session(
        id: Int,
        started: Long,
        state: String = AgentStates.WAITING_FOR_USER,
        paused: Boolean = false,
    ) = AgentSession(id = id, agent = "claude", started = started, state = state, paused = paused)

    private val target = Reply.Target(machine = "dev-1", session = 7, started = 1_000)

    @Test
    fun `a reply to the session it was written for is delivered`() {
        assertNull(Reply.check(target, "dev-1", listOf(session(7, 1_000))))
    }

    @Test
    fun `a session number reused after a prune does not receive the old session's reply`() {
        // The defect this whole Target type exists for. Session 7 was pruned
        // while the user was typing; the daemon handed 7 out again
        // (`registry.rs:441`) and a different agent now holds it. Sending
        // would type the user's sentence into a stranger's terminal.
        val refusal = Reply.check(target, "dev-1", listOf(session(7, 9_999)))
        assertEquals(Reply.Refusal.RECYCLED, refusal)
    }

    @Test
    fun `a recycled id is reported as recycled even when the new session has exited`() {
        // Order matters: identity is checked before liveness. Reported as
        // EXITED, the user would retry against the same number and hit the
        // same wrong agent the moment it was live.
        val refusal = Reply.check(target, "dev-1", listOf(session(7, 9_999, AgentStates.EXITED)))
        assertEquals(Reply.Refusal.RECYCLED, refusal)
    }

    @Test
    fun `a reply for another machine is refused rather than sent to this one`() {
        assertEquals(
            Reply.Refusal.OTHER_MACHINE,
            Reply.check(target, "laptop-2", listOf(session(7, 1_000))),
        )
    }

    @Test
    fun `a session the daemon no longer lists is gone, not recycled`() {
        assertEquals(Reply.Refusal.GONE, Reply.check(target, "dev-1", emptyList()))
    }

    @Test
    fun `an exited agent has no terminal to type into`() {
        assertEquals(
            Reply.Refusal.EXITED,
            Reply.check(target, "dev-1", listOf(session(7, 1_000, AgentStates.EXITED))),
        )
    }

    @Test
    fun `a paused agent is refused, because the bytes would sit unread`() {
        assertEquals(
            Reply.Refusal.PAUSED,
            Reply.check(target, "dev-1", listOf(session(7, 1_000, paused = true))),
        )
    }

    @Test
    fun `every refusal says what happened instead of failing silently`() {
        for (r in Reply.Refusal.entries) {
            assertTrue(r.message.length > 20, "${r.name} has no explanation")
        }
    }

    // ---- the bytes ------------------------------------------------------

    @Test
    fun `a reply is terminated with CR, because that is what the return key sends`() {
        // The daemon appends nothing: `session::write_input` writes raw bytes.
        // Without this the reply sits on the agent's input line unsubmitted,
        // which looks exactly like nothing having happened.
        assertEquals("yes\r", Reply.bytes("yes"))
    }

    @Test
    fun `a newline the keyboard added does not become a second empty line`() {
        // Android's IME puts \n in a multi-line field when the action key is
        // pressed. Forwarded as well as the CR, the agent gets the reply AND
        // a bare return, which many read as accepting the default for
        // whatever they ask next.
        assertEquals("yes\r", Reply.bytes("yes\n"))
        assertEquals("yes\r", Reply.bytes("yes\r\n"))
        assertEquals("yes\r", Reply.bytes("yes  "))
    }

    @Test
    fun `an empty reply is a bare return and is not refused`() {
        // "Press enter to continue" is exactly the case this workflow is for.
        assertEquals("\r", Reply.bytes(""))
        assertTrue(Reply.isBare("   "))
        assertTrue(!Reply.isBare("no"))
    }

    @Test
    fun `interior newlines survive, because a multi-line answer is one the user typed`() {
        assertEquals("one\ntwo\r", Reply.bytes("one\ntwo"))
    }

    @Test
    fun `the request is the wire shape the daemon parses`() {
        assertEquals("""{"cmd":"input","id":7,"data":"yes\r"}""", Agentd.input(7, Reply.bytes("yes")))
    }

    @Test
    fun `a reply containing a quote or a newline is escaped, not smuggled`() {
        // The wire refuses a literal newline in a frame in both directions: a
        // payload carrying its own terminator would put a second request in
        // one frame.
        val line = Agentd.input(1, Reply.bytes("say \"hi\"\nthen stop"))
        assertTrue(!line.contains('\n'), "a request line must never carry a literal newline")
        assertTrue(line.contains("""\"hi\""""), "quotes must be escaped, not dropped")
    }
}
