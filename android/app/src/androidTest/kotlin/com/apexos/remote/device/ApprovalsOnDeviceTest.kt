package com.apexos.remote.device

import com.apexos.remote.core.agent.AgentError
import com.apexos.remote.core.agent.MachineLink
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.filters.LargeTest
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import org.junit.runner.RunWith

/**
 * Approvals, from a phone, against a real `apex-agentd`.
 *
 * P1-060 C1 names **approvals** among the things an end-to-end test must
 * cover, and round 1's evidence named it as one of the three parts still
 * missing on a device. This is that part, and the shape it takes is decided by
 * a rule rather than by convenience.
 *
 * ## The rule: a phone can never approve or deny, and no setting changes that
 *
 * `apex-agentd/src/privilege.rs` refuses `decide` from any non-local origin,
 * and the check sits **before** the request is even looked up — so a phone
 * cannot deny either, and cannot learn from the refusal whether the request
 * exists. `apex-remoted` declares `claude-remote-control` on every connection
 * and `origin::may_declare` forbids narrowing back towards local, so there is
 * no sequence of frames from this phone that reaches an approval.
 *
 * `Approvals.kt` records that as a thing that deliberately does not exist —
 * there is no `decide` builder in `Agentd` — and the obvious next move on
 * seeing a list of pending approvals is to add a button. This file is the
 * on-device proof that the button would be refused: it builds the `decide`
 * frame by hand, sends it down the phone's own link, and asserts the daemon's
 * answer.
 *
 * ## What a phone CAN do, and is asserted doing
 *
 * See a real pending request with its origin and its reason intact; watch a
 * human at the computer decide it; and revoke. Revocation only ever removes
 * authority, which is why it carries no origin check at all.
 *
 * The request below is filed **by the computer**, through the two narrow
 * broker verbs `run-device-suite.sh` provides for it. That is not the phone
 * borrowing a local origin: it is the human this phone is not, doing the one
 * thing this phone may not do, so that there is something real on the screen
 * to assert about.
 */
@RunWith(AndroidJUnit4::class)
@LargeTest
class ApprovalsOnDeviceTest {

    /**
     * Send a line the app has no builder for, and read the daemon's own reply.
     *
     * `MachineLink.request` is the TRANSPORT: it returns whatever came back,
     * error replies included, and it is the `Agentd.readX` parsers above it
     * that turn `{"reply":"error"}` into an [AgentError]. So a `decide` sent
     * this way comes back as a string and a test that only checked for a thrown
     * exception would pass while the daemon approved the request — which is
     * exactly what the first version of this file did. The reply is read here
     * instead, with `org.json`, in the daemon's own words.
     */
    private fun raw(link: MachineLink, line: String): JSONObject =
        JSONObject(link.request(line, retry = false))

    @Test
    fun a_request_filed_at_the_computer_reaches_the_phone_with_its_reason_and_origin() {
        val reason = "device suite ${System.nanoTime()}: proving a request crosses to the phone"
        val filed = Desktop.filePrivilegeRequest("install", listOf("clang"), reason)
        val id = filed.getInt("id")

        val machine = Paired.machine("approvals-list-test")
        Paired.linkTo(machine).use { link ->
            val mine = link.requests().firstOrNull { it.id == id }
            assertNotNull(
                "the request the computer just filed must be visible from the phone; the " +
                    "phone saw ${link.requests().map { it.id }}",
                mine,
            )
            assertEquals("install", mine!!.verb)
            assertEquals(listOf("clang"), mine.packages)
            // The agent's own sentence, carried whole. A list that showed the
            // verb and dropped this would be a screen asking somebody to
            // authorise a root operation without telling them what it is for.
            assertEquals(reason, mine.reason)
            assertTrue("a request nobody has decided is pending", mine.isPending)
            // Filed by the broker, which is a process in a login session on the
            // computer — so the daemon recorded a human, not this phone.
            assertEquals("local-terminal", mine.requestOrigin)
        }
    }

    @Test
    fun the_phone_cannot_approve_a_real_pending_request_and_the_refusal_is_about_the_origin() {
        val filed = Desktop.filePrivilegeRequest(
            "install",
            listOf("clang"),
            "device suite ${System.nanoTime()}: this must not be approvable from a phone",
        )
        val id = filed.getInt("id")

        val machine = Paired.machine("approvals-refusal-test")
        Paired.linkTo(machine).use { link ->
            // Built by hand, because `Agentd` has no builder for it and must
            // not grow one. This is the frame the button somebody will one day
            // want to add would send.
            val reply = raw(link, """{"cmd":"decide","id":$id,"decision":"allow_once"}""")
            assertEquals("a phone approved a root operation on request $id: $reply", "error", reply.getString("reply"))
            assertEquals("permission_denied", reply.getString("kind"))
            val why = reply.getString("message")
            assertTrue(
                "the refusal must be about where the request came from, and say so: $why",
                why.contains("claude-remote-control") && why.contains("§7"),
            )

            // And it is still pending afterwards, which is the part that
            // matters: a refusal that had already written something down would
            // be worse than no refusal at all.
            assertTrue(
                "the request must still be waiting on a human",
                link.requests().first { it.id == id }.isPending,
            )
        }
    }

    @Test
    fun the_refusal_comes_before_the_lookup_so_a_phone_cannot_probe_for_request_ids() {
        val machine = Paired.machine("approvals-probe-test")
        Paired.linkTo(machine).use { link ->
            // An id that certainly does not exist. A LOCAL caller gets
            // `no_such_request` for this; a phone must get the same
            // `permission_denied` it gets for a real one, or the refusal is an
            // oracle for which request ids exist on somebody's computer.
            val reply = raw(link, """{"cmd":"decide","id":999999,"decision":"deny"}""")
            assertEquals("a phone decided request 999999, which does not exist: $reply", "error", reply.getString("reply"))
            assertEquals(
                "the answer for an absent request must be the answer for a present one",
                "permission_denied",
                reply.getString("kind"),
            )
        }
    }

    @Test
    fun a_human_at_the_computer_decides_and_the_phone_sees_the_decision() {
        val filed = Desktop.filePrivilegeRequest(
            "install",
            listOf("clang"),
            "device suite ${System.nanoTime()}: decided at the computer",
        )
        val id = filed.getInt("id")

        val machine = Paired.machine("approvals-decision-test")
        Paired.linkTo(machine).use { link ->
            assertTrue(
                "it starts pending",
                link.requests().first { it.id == id }.isPending,
            )
            Desktop.decideLocally(id, "deny")
            val after = link.requests().first { it.id == id }
            assertTrue("a decided request is no longer pending", !after.isPending)
            assertEquals("denied", after.decision)
        }
    }

    @Test
    fun a_phone_may_ask_what_has_been_granted_and_may_take_a_grant_away() {
        val machine = Paired.machine("approvals-grants-test")
        Paired.linkTo(machine).use { link ->
            // Both list verbs really answer. `grants` and `system_grants` are
            // different files with different lifetimes and a screen shows both.
            assertNotNull(link.grants())
            assertNotNull(link.systemGrants())

            // Revocation carries NO origin check — it only ever removes
            // authority — so what a phone gets back for a project with nothing
            // granted must be "I looked and there was nothing", and not "you
            // may not ask". The two are different answers and a screen that
            // could not tell them apart would tell somebody their phone is
            // locked out when the list is simply empty.
            try {
                link.revoke("/no/such/project/${System.nanoTime()}")
                fail("revoking a project with no grants reported success")
            } catch (e: AgentError) {
                assertEquals(
                    "a phone that may not revoke would be told permission_denied; it must be " +
                        "told the grant does not exist: ${e.message}",
                    "no_such_request",
                    e.kind,
                )
            }
            try {
                link.revokeSystemGrant(999999)
                fail("revoking system grant 999999 reported success")
            } catch (e: AgentError) {
                assertEquals("no_such_request", e.kind)
            }
        }
    }
}
