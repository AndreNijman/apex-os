package com.apexos.remote.ui

import com.apexos.remote.core.agent.AgentSession
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertNotNull
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Test

/**
 * Dismissing a banner has to actually clear the banner that is on screen.
 *
 * The Agent Center draws `state.failure ?: state.agents.failure` — two fields,
 * one strip — and `dismiss()` used to clear only the first. So a refusal
 * raised by `replyToSession`, which writes the second, came with a dismiss
 * button that cleared a field nobody was looking at and left the message
 * exactly where it was. Nothing caught it because nothing tested the state.
 *
 * This is the same family as the defect this project names "a gate that runs
 * and inspects nothing": the button ran, and it inspected nothing.
 */
class AgentUiStateTest {

    @Test
    fun `dismissing clears the failure the Agent Center is actually showing`() {
        // The one that was broken. `agents.failure` is what `replyToSession`,
        // `pullMachineClipboard` and every other agent-scoped action write.
        val state = AgentUiState(failure = "the machine refused")
        assertNull(state.dismissed().failure)
    }

    @Test
    fun `dismissing clears a notice, which is the strip the clipboard uses`() {
        // "l16's clipboard is empty" is not a failure and is drawn in the
        // ordinary colour, but it is still a banner a person wants gone. A
        // dismiss button beside a notice that dismiss() did not clear would
        // be a second button that does nothing.
        val state = AgentUiState(notice = "Copied 42 characters from l16.")
        assertNull(state.dismissed().notice)
    }

    @Test
    fun `dismissing does NOT drop a clipboard pull that is still in flight`() {
        // The distinction that makes `dismissed()` a considered method rather
        // than a blanket reset. A pull is not a banner: it is text the user
        // asked for, on its way to the phone's clipboard. Clearing it because
        // somebody tidied a message away during the round trip would lose the
        // thing they actually wanted, silently.
        val pull = ClipboardPull("ssh-ed25519 AAAA…", token = 7)
        val state = AgentUiState(notice = "a message", clipboardPull = pull)
        val after = state.dismissed()
        assertNull(after.notice)
        assertNotNull(after.clipboardPull)
        assertEquals(pull, after.clipboardPull)
    }

    @Test
    fun `dismissing keeps everything that is not a banner`() {
        // A dismiss that quietly emptied the session list would send the
        // screen to its "no agents" state, which claims a fact about the
        // machine rather than reporting one.
        val sessions = listOf(
            AgentSession(id = 1, agent = "claude", state = "working", cwd = "/p"),
        )
        val state = AgentUiState(sessions = sessions, failure = "x", notice = "y")
        val after = state.dismissed()
        assertEquals(sessions, after.sessions)
        assertEquals(state.nowSeconds, after.nowSeconds)
    }

    @Test
    fun `dismissing a state with nothing on it changes nothing`() {
        // Idempotent, and asserted rather than assumed: the button is
        // reachable whether or not a banner is up.
        val state = AgentUiState()
        assertEquals(state, state.dismissed())
        assertEquals(state.dismissed(), state.dismissed().dismissed())
    }
}
