package com.apexos.remote.core.agent

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

/**
 * The approval surface, and the one thing it must never offer.
 *
 * The shapes here were read out of `apex-agent-core` rather than guessed, and
 * the ones that would fail silently are asserted by name: kebab-case origins
 * beside snake_case decisions, `pkg_upgrade` on the wire against `pkg-upgrade`
 * in a grant key, `packages` absent rather than null, and `states` as an array
 * of arrays.
 */
class ApprovalsTest {
    // ---- the verb that must not exist -----------------------------------

    @Test
    fun `there is no decide builder, because the daemon refuses it from a phone`() {
        // `privilege.rs:1176` refuses `decide` from any non-local origin,
        // before the pending check, so a phone cannot approve OR deny. This is
        // the inverted test: it fails the moment somebody adds the builder,
        // which is the obvious thing to do on seeing a list of pending
        // approvals and is exactly what must not happen.
        val builders = Agentd::class.java.methods.map { it.name }.toSet()
        assertFalse("decide" in builders, "Agentd grew a `decide` builder; see Approvals.kt")
        assertFalse("readDecision" in builders)
        // And the sentence shown instead is the daemon's own, not a paraphrase.
        assertTrue(PrivilegeRequest.DECIDE_IS_LOCAL_ONLY.contains("Approve this at the machine"))
        assertTrue(PrivilegeRequest.DECIDE_IS_LOCAL_ONLY.contains("no setting that allows it"))
    }

    // ---- requests --------------------------------------------------------

    private val installReply = """
        {"reply":"requests","requests":[
          {"id":3,"verb":"install","packages":["clang","cmake"],
           "reason":"Required to compile the project","session":null,
           "agent":"claude","project":"/home/u/proj",
           "request_origin":"claude-remote-control","origin_source":"declared",
           "actor":"pixel8office1234","decision":"pending",
           "created_ms":1757660000000,"decided_ms":null,"executed_ms":null,
           "exit_code":null,"system_grant":null},
          {"id":4,"verb":"pkg_upgrade",
           "reason":"security updates","session":12,"agent":"codex",
           "project":"/home/u/proj","request_origin":"local-terminal",
           "origin_source":"observed","actor":null,
           "decision":"allow_for_project","created_ms":1757660100000,
           "decided_ms":1757660150000,"executed_ms":null,"exit_code":null,
           "system_grant":7}
        ]}
    """.trimIndent()

    @Test
    fun `a privilege request is read with its flattened verb`() {
        val rs = Agentd.readRequests(installReply)
        assertEquals(2, rs.size)
        val install = rs[0]
        assertEquals(3, install.id)
        // The verb is flattened: it sits BESIDE id, not under a `verb` object.
        assertEquals("install", install.verb)
        assertEquals(listOf("clang", "cmake"), install.packages)
        assertEquals("Required to compile the project", install.reason)
        assertNull(install.session)
        assertEquals("claude", install.agent)
        assertEquals("/home/u/proj", install.project)
        assertTrue(install.isPending)
    }

    @Test
    fun `packages is absent for an arg-less verb and does not become null`() {
        // The six arg-less verbs are enum variants with no fields, so the key
        // is missing rather than null. An empty list is the only correct
        // reading; a nullable field would make every screen ask twice.
        val upgrade = Agentd.readRequests(installReply)[1]
        assertEquals("pkg_upgrade", upgrade.verb)
        assertTrue(upgrade.packages.isEmpty())
    }

    @Test
    fun `the wire verb is underscored and the label is hyphenated`() {
        // `Request::PrivilegeRequest.verb` is snake_case on the wire, while
        // the SAME operation is `pkg-upgrade` in a grant key and in
        // SystemGrant.capabilities. Both appear on one screen, and a user
        // seeing `pkg_upgrade` pending beside a `pkg-upgrade` grant would read
        // them as two different operations.
        val upgrade = Agentd.readRequests(installReply)[1]
        assertEquals("pkg_upgrade", upgrade.verb)
        assertEquals("pkg-upgrade", upgrade.verbLabel)
        assertEquals("install", Agentd.readRequests(installReply)[0].verbLabel)
    }

    @Test
    fun `origins are kebab-case and decisions are snake_case`() {
        // Two vocabularies in one record. `RequestOrigin` is
        // `rename_all = "kebab-case"`; `Decision` is snake_case. A single
        // converter over both would corrupt one of them.
        val rs = Agentd.readRequests(installReply)
        assertEquals("claude-remote-control", rs[0].requestOrigin)
        assertEquals("declared", rs[0].originSource)
        assertEquals("pending", rs[0].decision)
        assertEquals("local-terminal", rs[1].requestOrigin)
        assertEquals("allow_for_project", rs[1].decision)
    }

    @Test
    fun `a remote-filed request is told apart from a local one`() {
        val rs = Agentd.readRequests(installReply)
        assertTrue(rs[0].fromRemote) // claude-remote-control
        assertFalse(rs[1].fromRemote) // local-terminal
        // apex-shell is the OTHER local origin, and a screen that forgot it
        // would label the desktop's own Agent Center as a remote actor.
        assertFalse(
            PrivilegeRequest(requestOrigin = "apex-shell").fromRemote,
        )
        // An origin this build has never heard of is not assumed local.
        assertTrue(PrivilegeRequest(requestOrigin = "cloud-job").fromRemote)
        // No origin at all is not remote and not local: it is nothing, and a
        // record written before origin tracking existed reads that way.
        assertFalse(PrivilegeRequest(requestOrigin = null).fromRemote)
    }

    @Test
    fun `allowed does not mean it ran`() {
        // `request.rs:410`: AllowForProject does NOT execute — a granted
        // request still waits for `apex request approve` at the machine,
        // because execution goes through the approving human's privilege. A
        // phone drawing "allowed" as "done" tells the user work happened that
        // has not.
        val granted = Agentd.readRequests(installReply)[1]
        assertTrue(granted.isAllowed)
        assertNull(granted.executedMs)
        assertTrue(granted.awaitingExecution)

        val ran = granted.copy(executedMs = 1757660200000, exitCode = 0)
        assertTrue(ran.isAllowed)
        assertFalse(ran.awaitingExecution)
        // Pending is not awaiting EXECUTION; it is awaiting a decision.
        assertFalse(Agentd.readRequests(installReply)[0].awaitingExecution)
    }

    @Test
    fun `every verb in the vocabulary has a sentence and an unknown one is named`() {
        val known = listOf(
            "install", "remove", "pkg_upgrade", "pkg_rebuild",
            "pkg_rollback", "pin", "rollback", "update",
        )
        for (v in known) {
            val e = PrivilegeRequest(verb = v, packages = listOf("x")).effect
            assertFalse(e.contains("`"), "$v fell through to the unknown-verb sentence")
            assertTrue(e.isNotEmpty())
        }
        // A verb added to the daemon after this build is NAMED rather than
        // described, because inventing a sentence for a root operation this
        // app has never heard of would be describing what it cannot describe.
        assertEquals("run the `quarantine` operation", PrivilegeRequest(verb = "quarantine").effect)
    }

    // ---- grants ----------------------------------------------------------

    @Test
    fun `per-project grants are read as project root to grant keys`() {
        val g = Agentd.readGrants(
            """{"reply":"grants","projects":{"/home/u/proj":["install:clang","pin"],"/home/u/b":[]}}""",
        )
        assertEquals(setOf("/home/u/proj", "/home/u/b"), g.keys)
        assertEquals(listOf("install:clang", "pin"), g["/home/u/proj"])
        assertTrue(g["/home/u/b"]!!.isEmpty())
    }

    @Test
    fun `revoking without a key omits it rather than sending null`() {
        // `Request::Revoke.key` is `#[serde(default)]`, and an ABSENT key
        // revokes every grant for the project. Sending `"key":null` happens to
        // mean the same to serde — but the wide form must be the one a caller
        // asked for, not one a serialiser produced.
        assertEquals(
            """{"cmd":"revoke","project":"/home/u/p"}""",
            Agentd.revoke("/home/u/p"),
        )
        assertEquals(
            """{"cmd":"revoke","project":"/home/u/p","key":"install:clang"}""",
            Agentd.revoke("/home/u/p", "install:clang"),
        )
        assertEquals("""{"cmd":"revoke_system_grant","id":4}""", Agentd.revokeSystemGrant(4))
        assertEquals("""{"cmd":"requests"}""", Agentd.requests())
        assertEquals("""{"cmd":"grants"}""", Agentd.grants())
        assertEquals("""{"cmd":"system_grants"}""", Agentd.systemGrants())
    }

    // ---- system grants ---------------------------------------------------

    private val systemReply = """
        {"reply":"system_grants",
         "grants":[
           {"id":4,"kind":"system_access","session":12,"agent":"claude",
            "project":"/home/u/proj","capabilities":["install","pin"],
            "issued_ms":1757660000000,"expires_ms":1757660900000,
            "boot_id":"abc","request_origin":"local-terminal",
            "authenticated_by":"org.apexos.agent.system-access","closed":null},
           {"id":5,"kind":"break_glass","session":13,"agent":"codex",
            "project":null,"capabilities":[],
            "issued_ms":1757650000000,"expires_ms":1757650060000,
            "boot_id":"abc","request_origin":"claude-remote-control",
            "authenticated_by":"org.apexos.agent.break-glass",
            "closed":{"ms":1757650050000,"why":"revoked"}}
         ],
         "states":[["active","14m left"],["ended-at-reboot","ended at reboot"]]}
    """.trimIndent()

    @Test
    fun `system grants pair with the state the daemon computed`() {
        val gs = Agentd.readSystemGrants(systemReply)
        assertEquals(2, gs.size)
        assertEquals(4, gs[0].first.id)
        assertEquals("active", gs[0].second.word)
        assertEquals("14m left", gs[0].second.sentence)
        assertEquals(listOf("install", "pin"), gs[0].first.capabilities)
        assertTrue(gs[0].first.isOpen)
        assertFalse(gs[0].first.isBreakGlass)
    }

    @Test
    fun `states is an array of arrays and not of objects`() {
        // serde serializes a Rust (String, String) as a two-element ARRAY.
        // Decoding it as objects throws; decoding it as a flat list of strings
        // silently pairs every grant with the wrong half of its neighbour's.
        val gs = Agentd.readSystemGrants(systemReply)
        assertEquals("ended-at-reboot", gs[1].second.word)
        assertEquals("ended at reboot", gs[1].second.sentence)
    }

    @Test
    fun `the closure vocabulary is not the state vocabulary`() {
        // `closed.why` is `reboot`/`runtime_restart`; the display word for the
        // same concept is `ended-at-reboot`/`ended-with-the-runtime`. Sharing
        // one Kotlin enum across both would have to fail on one of them.
        val closed = Agentd.readSystemGrants(systemReply)[1].first.closed!!
        assertEquals("revoked", closed.why)
        assertEquals(1757650050000L, closed.ms)
        assertFalse(Agentd.readSystemGrants(systemReply)[1].first.isOpen)
        assertTrue(Agentd.readSystemGrants(systemReply)[1].first.isBreakGlass)
    }

    @Test
    fun `a grant with no state is shown rather than dropped`() {
        // A live root grant missing from a list the user is reading is the
        // worst outcome available on this screen, so a short `states` costs
        // the sentence and never the row.
        val short = """
            {"reply":"system_grants",
             "grants":[{"id":4,"kind":"system_access","session":1,"agent":"c",
               "project":null,"capabilities":[],"issued_ms":0,"expires_ms":0,
               "boot_id":"b","request_origin":"local-terminal",
               "authenticated_by":"x","closed":null}],
             "states":[]}
        """.trimIndent()
        val gs = Agentd.readSystemGrants(short)
        assertEquals(1, gs.size)
        assertEquals("", gs[0].second.word)
    }

    @Test
    fun `an error reply throws rather than reading as no approvals pending`() {
        // The failure this guards is the dangerous direction: a refusal shown
        // as an empty list tells the user nothing needs their attention.
        for (parse in listOf<(String) -> Any>(
            { Agentd.readRequests(it) },
            { Agentd.readGrants(it) },
            { Agentd.readSystemGrants(it) },
        )) {
            var threw = false
            try {
                parse("""{"reply":"error","kind":"permission_denied","message":"no"}""")
            } catch (e: AgentError) {
                threw = true
                assertEquals("permission_denied", e.kind)
            }
            assertTrue(threw, "a refusal parsed as an empty list")
        }
    }
}
