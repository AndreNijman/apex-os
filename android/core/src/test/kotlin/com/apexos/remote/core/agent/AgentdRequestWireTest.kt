package com.apexos.remote.core.agent

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonObject
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

/**
 * Every request this app builds, against a fixture the daemon's serde parses.
 *
 * This half asserts the builders produce the fixture. The other half —
 * `apexd/apex-agent-core/tests/android_requests_wire.rs` — deserializes the
 * same file into `Request` and asserts which variant each becomes. Neither
 * half alone is worth much: a Kotlin test comparing a builder to a string a
 * Kotlin author wrote proves the two agree with each other, and that is
 * exactly the failure this unit has already had twice. The fixture being
 * `Request`'s own input is what makes it evidence.
 *
 * Compared as parsed JSON rather than as text: key order and spacing are not
 * the contract, and a comparison that failed on reformatting would be
 * retrained away the first time somebody tidied the file.
 */
class AgentdRequestWireTest {
    private val json = Json { ignoreUnknownKeys = false }

    private val fixture: JsonObject by lazy {
        val stream = javaClass.classLoader.getResourceAsStream("requests.json")
            ?: error("the shared request fixture is missing from the test resources")
        json.parseToJsonElement(stream.bufferedReader().readText()).jsonObject
    }

    private fun expect(name: String, built: String) {
        val want = fixture[name] ?: error("the fixture has no request called `$name`")
        assertEquals(
            want,
            json.parseToJsonElement(built),
            "`$name` is not the request the daemon's own fixture says it is",
        )
        // A request travels as ONE line. `apex-remote-core`'s wire refuses an
        // embedded newline in both directions, because a payload carrying its
        // own terminator would let a client put a second request in one frame.
        assertFalse(built.contains('\n'), "`$name` carries a literal newline")
    }

    @Test
    fun `the session verbs are the shapes the daemon parses`() {
        expect("hello", Agentd.hello())
        expect("list", Agentd.list())
        expect("info", Agentd.info(7))
        expect("attach", Agentd.attach(7, 80, 24))
        expect("resize", Agentd.resize(7, 100, 40))
        expect("signal", Agentd.interrupt(7))
    }

    @Test
    fun `input is a verb, and it is the shape write_input reads`() {
        // The reply carries its own CR: `session::write_input` writes raw bytes
        // and appends nothing, so a reply without one is never submitted.
        expect("input", Agentd.input(7, Reply.bytes("yes")))
    }


    @Test
    fun `run omits what it has no value for, rather than sending nulls`() {
        // `RunRequest`'s optional fields are `#[serde(default)]`, and a key
        // holding null is not the same as an absent key to every one of them.
        expect("run_minimal", Agentd.run("/home/andre/Projects/apex", 80, 24))
    }

    @Test
    fun `run carries the checkpoint flag only when one was asked for`() {
        expect(
            "run_full",
            Agentd.run(
                cwd = "/home/andre/Projects/apex",
                cols = 80,
                rows = 24,
                agent = "claude",
                prompt = "review the diff",
                worktree = "wt-review",
                checkpoint = true,
            ),
        )
        // False is the daemon's default and the absence means the same thing —
        // but a daemon predating the field rejects the key outright, and this
        // app runs against whatever the machine has.
        assertFalse(
            Agentd.run("/x", 80, 24, checkpoint = false).contains("checkpoint"),
            "a request that wants no checkpoint must not mention one",
        )
    }

    @Test
    fun `worktrees is a verb — the note that said it was not read the wrong enum`() {
        expect("worktrees_all", Agentd.worktrees())
        expect("worktrees_one", Agentd.worktrees("apex"))
    }

    @Test
    fun `the approval verbs a phone is allowed`() {
        expect("requests", Agentd.requests())
        expect("grants", Agentd.grants())
        expect("system_grants", Agentd.systemGrants())
        expect("revoke_one", Agentd.revoke("/home/andre/Projects/apex", "pkg-upgrade"))
        expect("revoke_all", Agentd.revoke("/home/andre/Projects/apex"))
        expect("revoke_system_grant", Agentd.revokeSystemGrant(3))
    }

    @Test
    fun `omitting a grant key means every grant for the project, and is not a null`() {
        // `Request::Revoke.key` is `Option<String>` with `#[serde(default)]`,
        // and absent means "everything for this project" — a wide action. A
        // caller that meant one key and sent a null would revoke them all.
        assertFalse(Agentd.revoke("/p").contains("key"), "a wide revoke must not name a key")
        assertTrue(Agentd.revoke("/p", "pin").contains(""""key":"pin""""))
    }

    @Test
    fun `nothing here builds a decide request`() {
        // Inverted on purpose, and it is the one assertion in this file that
        // is about something NOT existing. `privilege.rs:1176` refuses
        // `decide` from any non-local origin BEFORE the pending check, so a
        // paired phone can neither approve nor deny, and `Request::Decide`
        // carries no second factor. Adding the builder is the obvious move on
        // seeing a list of pending approvals; it would be refused every time
        // it was pressed. If this test ever fails, read `Approvals.kt` before
        // deleting it.
        val builders = Agentd::class.java.methods.map { it.name }
        assertFalse("decide" in builders, "a decide builder was added; see Approvals.kt")
        assertFalse(
            fixture.keys.any { it.contains("decide") },
            "the shared request fixture grew a decide request",
        )
    }

    @Test
    fun `the fixture and the builders cover the same set`() {
        // So that a request added to one half and not the other is caught,
        // rather than the fixture quietly drifting into a list of things
        // nothing sends.
        val named = fixture.keys.filterNot { it.startsWith("_") }.toSet()
        assertEquals(
            setOf(
                "hello", "list", "info", "attach", "resize", "signal", "input",
                "run_minimal", "run_full", "worktrees_all", "worktrees_one",
                "requests", "grants", "system_grants",
                "revoke_one", "revoke_all", "revoke_system_grant",
            ),
            named,
            "the fixture no longer matches the requests this test exercises",
        )
    }
}
