package com.apexos.remote.ui.agent

import com.apexos.remote.core.agent.PrivilegeRequest
import com.apexos.remote.ui.ApprovalsUiState
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

/**
 * The rules on the approvals screen that are not drawing.
 *
 * Compose itself is untestable here — no device, no emulator, no Robolectric —
 * so the decisions that matter are pulled out as ordinary functions and tested
 * as ordinary functions. What is left inside the composables is layout.
 */
class ApprovalsTest {

    private fun request(
        decision: String,
        executedMs: Long? = null,
        exitCode: Int? = null,
    ) = PrivilegeRequest(
        id = 1,
        verb = "pkg_upgrade",
        decision = decision,
        executedMs = executedMs,
        exitCode = exitCode,
    )

    @Test
    fun `an allowed request that has not run is not shown as done`() {
        // The distinction P1-057 turns on. `request.rs:410`: an
        // `allow_for_project` decision does NOT execute anything — the
        // operation still goes through the approving human's own privilege at
        // the machine. A row reading "allowed" as "done" tells the user work
        // has happened that has not, and the thing they then do not do is the
        // thing that makes it happen.
        val line = decisionLine(request(PrivilegeRequest.ALLOW_FOR_PROJECT))
        assertTrue(line.contains("not run yet"), "an unexecuted allowance read as complete: $line")
        assertTrue(line.contains("at the machine"), "it must say where to go: $line")
    }

    @Test
    fun `the same is true of a one-shot allowance`() {
        assertTrue(decisionLine(request(PrivilegeRequest.ALLOW_ONCE)).contains("not run yet"))
    }

    @Test
    fun `an allowed request that ran says so, and one that failed says how`() {
        assertEquals(
            "allowed, and it ran",
            decisionLine(request(PrivilegeRequest.ALLOW_ONCE, executedMs = 5, exitCode = 0)),
        )
        assertEquals(
            "allowed, and it failed with exit 1",
            decisionLine(request(PrivilegeRequest.ALLOW_ONCE, executedMs = 5, exitCode = 1)),
        )
    }

    @Test
    fun `a request that ran with no exit code recorded is not claimed to have succeeded`() {
        // `exit_code` is optional on the wire. Absent, this must not fall
        // through to the "it ran" sentence, which asserts a zero nobody sent.
        assertEquals("allowed", decisionLine(request(PrivilegeRequest.ALLOW_ONCE, executedMs = 5)))
    }

    @Test
    fun `pending says where the decision has to be made`() {
        val line = decisionLine(request(PrivilegeRequest.PENDING))
        assertTrue(line.contains("at the machine"), "a phone cannot decide, and the row must say so")
    }

    @Test
    fun `denied is denied`() {
        assertEquals("denied", decisionLine(request(PrivilegeRequest.DENIED)))
    }

    // ---- what a confirmation says before it removes authority -----------

    @Test
    fun `revoking every grant for a project says that it is every grant`() {
        val wide = Revocation.Project("/home/andre/Projects/apex", null)
        assertTrue(wide.title.contains("every"), wide.title)
        assertTrue(wide.explanation.contains("Every standing permission"), wide.explanation)
        // `Request::Revoke.key` absent means all of them, which is the wide
        // action — a confirmation naming one key would be describing a
        // narrower thing than the one about to happen.
        assertFalse(wide.explanation.contains("pkg-upgrade"))
    }

    @Test
    fun `a narrow revoke names the operation and not the whole project`() {
        val one = Revocation.Project("/home/andre/Projects/apex", "pkg-upgrade")
        assertTrue(one.title.contains("pkg-upgrade"), one.title)
        assertFalse(one.explanation.contains("Every standing permission"), one.explanation)
    }

    @Test
    fun `every confirmation says that it is not retried`() {
        // Revoke is deliberately not retried: a lost reply after a wide revoke
        // would have the retry answer `no_such_request`, reporting failure for
        // something that succeeded. The user is told before they press it, not
        // after.
        val all = listOf(
            Revocation.Project("/p", null),
            Revocation.Project("/p", "pin"),
            Revocation.System(3),
        )
        for (r in all) {
            assertTrue(
                r.explanation.contains("Not retried") || r.explanation.contains("not retried"),
                "${r.title} does not say it is not retried",
            )
        }
    }

    @Test
    fun `ending system access says what it costs, not just that it ends`() {
        val system = Revocation.System(3)
        assertTrue(system.explanation.contains("immediately"), system.explanation)
        assertTrue(system.explanation.contains("will fail"), system.explanation)
    }

    // ---- the state the screen reads -------------------------------------

    @Test
    fun `pending is only the undecided ones`() {
        val state = ApprovalsUiState(
            requests = listOf(
                request(PrivilegeRequest.PENDING),
                request(PrivilegeRequest.DENIED),
                request(PrivilegeRequest.ALLOW_ONCE),
            ),
        )
        assertEquals(1, state.pending.size)
    }

    @Test
    fun `a failure is not an empty list`() {
        // A refusal drawn as "nothing is waiting" tells the user that nothing
        // needs their attention, which is the opposite of what happened.
        val state = ApprovalsUiState(failure = "permission denied")
        assertTrue(state.requests.isEmpty())
        assertTrue(state.failure != null, "the screen must be able to tell the two apart")
    }
}
