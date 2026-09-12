package com.apexos.remote.core.agent

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

/**
 * Alerts fire on transitions, once, and carry nothing sensitive.
 */
class AlertsTest {
    private fun session(id: Int, state: String, agent: String = "claude", detail: String? = null) =
        AgentSession(id = id, state = state, agent = agent, detail = detail, cwd = "/home/u/secret-project")

    private fun watcher() = AlertWatcher()

    // ---- the first poll --------------------------------------------------

    @Test
    fun `the first poll of a machine raises nothing`() {
        // Opening the app to six sessions that have been waiting since
        // yesterday must not fire six notifications for things that did not
        // just happen. This is also the state after a process restart, which
        // is precisely when the burst would be worst.
        val w = watcher()
        val alerts = w.observe(
            "m1",
            listOf(
                session(1, AgentStates.WAITING_FOR_USER),
                session(2, AgentStates.PERMISSION_REQUEST),
                session(3, AgentStates.FAILED),
            ),
        )
        assertTrue(alerts.isEmpty())
    }

    @Test
    fun `a transition after the first poll raises exactly one alert`() {
        val w = watcher()
        w.observe("m1", listOf(session(1, AgentStates.WORKING)))
        val alerts = w.observe("m1", listOf(session(1, AgentStates.WAITING_FOR_USER)))
        assertEquals(1, alerts.size)
        assertEquals(Alert.Kind.WAITING, alerts[0].kind)
        assertEquals(1, alerts[0].session)
        assertEquals("m1", alerts[0].machine)
        assertEquals("claude", alerts[0].agent)
    }

    // ---- the dedup that P1-058 actually asks for -------------------------

    @Test
    fun `four reports of the same waiting state raise one alert`() {
        // THE DUPLICATE IS INTERNAL TO APEX, not Claude-versus-APEX. Four
        // paths publish `waiting_for_user` for one turn — the `notification`
        // hook, the `stop` hook, the PTY BEL/OSC scanner and the ten-second
        // idle rule — and `Session::set_state` (registry.rs:163) has no
        // same-state early return, so each one rewrites the record and bumps
        // last_activity. A poller keying on "saw waiting_for_user" fires four
        // times, seconds apart, for one event.
        val w = watcher()
        w.observe("m1", listOf(session(1, AgentStates.WORKING)))
        val first = w.observe("m1", listOf(session(1, AgentStates.WAITING_FOR_USER)))
        assertEquals(1, first.size)
        repeat(3) {
            // Same state, a new `detail` and a new last_activity each time:
            // exactly what the other three paths produce.
            assertTrue(
                w.observe(
                    "m1",
                    listOf(session(1, AgentStates.WAITING_FOR_USER, detail = "hook $it")),
                ).isEmpty(),
            )
        }
    }

    @Test
    fun `leaving and re-entering a state alerts again`() {
        // The rule is the transition edge, not "once ever". A second turn
        // finishing is a second thing to know about.
        val w = watcher()
        w.observe("m1", listOf(session(1, AgentStates.WORKING)))
        assertEquals(1, w.observe("m1", listOf(session(1, AgentStates.WAITING_FOR_USER))).size)
        assertTrue(w.observe("m1", listOf(session(1, AgentStates.WORKING))).isEmpty())
        assertEquals(1, w.observe("m1", listOf(session(1, AgentStates.WAITING_FOR_USER))).size)
    }

    @Test
    fun `working and starting are not news`() {
        val w = watcher()
        w.observe("m1", listOf(session(1, AgentStates.STARTING)))
        assertTrue(w.observe("m1", listOf(session(1, AgentStates.WORKING))).isEmpty())
        assertTrue(w.observe("m1", listOf(session(1, AgentStates.STARTING))).isEmpty())
    }

    @Test
    fun `two machines with a session 1 each do not deduplicate each other`() {
        // `SessionInfo.id` is a per-daemon counter, so every paired machine
        // has a session 1. A key without the machine would silence the second.
        val w = watcher()
        w.observe("m1", listOf(session(1, AgentStates.WORKING)))
        w.observe("m2", listOf(session(1, AgentStates.WORKING)))
        val a = w.observe("m1", listOf(session(1, AgentStates.WAITING_FOR_USER)))
        val b = w.observe("m2", listOf(session(1, AgentStates.WAITING_FOR_USER)))
        assertEquals(1, a.size)
        assertEquals(1, b.size)
        assertEquals("m1", a[0].machine)
        assertEquals("m2", b[0].machine)
        assertFalse(a[0].key == b[0].key)
    }

    @Test
    fun `a recycled session id is not compared against its predecessor`() {
        // `Request::Remove` and `Prune` delete the record AND the log, and
        // `reserve_id` (registry.rs:441) then hands the id out again. A new
        // session landing on a recycled id must not be diffed against a
        // stranger's last state — which here would SUPPRESS its alert.
        val w = watcher()
        w.observe("m1", listOf(session(1, AgentStates.WORKING)))
        w.observe("m1", listOf(session(1, AgentStates.WAITING_FOR_USER)))
        // Pruned: the daemon no longer lists it.
        assertTrue(w.observe("m1", emptyList()).isEmpty())
        // A different session, same id, arriving already waiting.
        val alerts = w.observe("m1", listOf(session(1, AgentStates.WAITING_FOR_USER)))
        assertEquals(1, alerts.size)
    }

    @Test
    fun `every state that means something maps to a kind`() {
        val w = watcher()
        for ((state, kind) in listOf(
            AgentStates.WAITING_FOR_USER to Alert.Kind.WAITING,
            AgentStates.PERMISSION_REQUEST to Alert.Kind.PERMISSION,
            AgentStates.FAILED to Alert.Kind.FAILED,
            AgentStates.COMPLETE to Alert.Kind.FINISHED,
            AgentStates.EXITED to Alert.Kind.FINISHED,
        )) {
            w.forget("m")
            w.observe("m", listOf(session(1, AgentStates.WORKING)))
            assertEquals(kind, w.observe("m", listOf(session(1, state))).single().kind, state)
        }
    }

    // ---- test failures ---------------------------------------------------

    @Test
    fun `a test run going red raises an alert against the worktree's session`() {
        // Not a session state: a TestNote reaches the daemon on a hook event
        // and is stored per WORKTREE PATH, surfacing on WorktreeStatus.tests.
        // So this comes from a `worktrees` poll and not from `list`.
        val w = watcher()
        val running = WorktreeStatus(
            name = "wt", path = "/p/wt", isAgent = true, sessions = listOf(9),
            tests = TestStatus(state = TestStatus.RUNNING, command = "cargo test"),
        )
        w.observe("m1", listOf(session(9, AgentStates.WORKING)), listOf(running))
        val failed = running.copy(tests = TestStatus(state = TestStatus.FAILED, command = "cargo test"))
        val alerts = w.observe("m1", listOf(session(9, AgentStates.WORKING)), listOf(failed))
        assertEquals(1, alerts.size)
        assertEquals(Alert.Kind.TEST_FAILED, alerts[0].kind)
        assertEquals(9, alerts[0].session)
        // And not again while it stays red.
        assertTrue(w.observe("m1", listOf(session(9, AgentStates.WORKING)), listOf(failed)).isEmpty())
    }

    @Test
    fun `a failing worktree with no session attached still alerts`() {
        val w = watcher()
        val running = WorktreeStatus(
            name = "wt", path = "/p/wt", isAgent = true,
            tests = TestStatus(state = TestStatus.RUNNING),
        )
        w.observe("m1", emptyList(), listOf(running))
        val alerts = w.observe(
            "m1", emptyList(),
            listOf(running.copy(tests = TestStatus(state = TestStatus.FAILED))),
        )
        assertEquals(Alert.NO_SESSION, alerts.single().session)
    }

    @Test
    fun `a passing test raises nothing`() {
        val w = watcher()
        val running = WorktreeStatus(name = "wt", path = "/p/wt", tests = TestStatus(state = TestStatus.RUNNING))
        w.observe("m1", emptyList(), listOf(running))
        assertTrue(
            w.observe(
                "m1", emptyList(),
                listOf(running.copy(tests = TestStatus(state = TestStatus.PASSED))),
            ).isEmpty(),
        )
    }

    // ---- approvals -------------------------------------------------------

    @Test
    fun `a pending privilege request alerts once and a decided one does not`() {
        val w = watcher()
        w.observe("m1", emptyList())
        val pending = PrivilegeRequest(id = 3, verb = "install", decision = PrivilegeRequest.PENDING, session = 5, agent = "claude")
        val alerts = w.observe("m1", emptyList(), emptyList(), listOf(pending))
        assertEquals(1, alerts.size)
        assertEquals(Alert.Kind.APPROVAL, alerts[0].kind)
        assertEquals(5, alerts[0].session)
        // Polled again, unchanged.
        assertTrue(w.observe("m1", emptyList(), emptyList(), listOf(pending)).isEmpty())
        // Decided at the machine — and never announced again.
        val decided = pending.copy(decision = PrivilegeRequest.ALLOW_ONCE)
        assertTrue(w.observe("m1", emptyList(), emptyList(), listOf(decided)).isEmpty())
    }

    // ---- what an alert may carry ----------------------------------------

    @Test
    fun `an alert carries nothing that names the work`() {
        // `SessionInfo.detail` copies Bash command lines, file paths, grep
        // patterns, fetched URLs and Claude's own notification text verbatim
        // (hook.rs:437). `cwd`, `project`, `worktree`, `args` and
        // `telemetry.branch` name what the user is working on. None of them
        // may be in an Alert, and this asserts on the RENDERED text rather
        // than on the field list, because a field added later would slip past
        // a list.
        val w = watcher()
        w.observe("m1", listOf(session(1, AgentStates.WORKING)))
        val a = w.observe(
            "m1",
            listOf(
                session(1, AgentStates.WAITING_FOR_USER, detail = "rm -rf /home/u/secret-project/x"),
            ),
        ).single()
        val rendered = "${a.machine} ${a.session} ${a.agent} ${a.kind.title} ${a.kind.detail}"
        assertFalse(rendered.contains("secret-project"))
        assertFalse(rendered.contains("rm -rf"))
        assertFalse(rendered.contains("/home/u"))
    }

    @Test
    fun `every kind has fixed wording that no reply can influence`() {
        // The words come from this build, not from the daemon, so a hostile or
        // merely verbose `detail` cannot reach a notification shade.
        for (k in Alert.Kind.entries) {
            assertTrue(k.title.isNotEmpty())
            assertTrue(k.detail.isNotEmpty())
            assertFalse(k.title.contains("{"))
        }
        assertEquals("Waiting for you", Alert.Kind.WAITING.title)
    }

    @Test
    fun `forgetting a machine makes its next poll a first poll`() {
        val w = watcher()
        w.observe("m1", listOf(session(1, AgentStates.WORKING)))
        w.forget("m1")
        assertTrue(w.observe("m1", listOf(session(1, AgentStates.WAITING_FOR_USER))).isEmpty())
    }
}
