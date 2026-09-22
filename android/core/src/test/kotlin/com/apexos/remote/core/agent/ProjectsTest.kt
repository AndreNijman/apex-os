package com.apexos.remote.core.agent

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertNotNull
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertThrows
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

/**
 * The phone reads real `projects` and `profiles` replies.
 *
 * `projects-reply.json` and `profiles-reply.json` are SHARED fixtures:
 * `apex-agent-core`'s `tests/android_projects_wire.rs` deserializes the same
 * two files into its own `Response` and asserts that re-serializing each
 * produces the identical JSON value. So this suite passing means the phone
 * parses what the daemon sends, rather than what somebody wrote down here.
 *
 * Either half alone proves nothing, and this unit has the scar: two rounds of
 * work were built on a claim about which verbs exist, read off the wrong enum,
 * with no test anywhere that could contradict it.
 */
class ProjectsTest {
    private fun projectsReply(): String =
        ProjectsTest::class.java.getResourceAsStream("/projects-reply.json")
            ?.bufferedReader()?.readText()
            ?: error("projects-reply.json is missing from core/src/test/resources/")

    private fun profilesReply(): String =
        ProjectsTest::class.java.getResourceAsStream("/profiles-reply.json")
            ?.bufferedReader()?.readText()
            ?: error("profiles-reply.json is missing from core/src/test/resources/")

    private fun projects(): List<ProjectRecord> = Agentd.readProjects(projectsReply())

    private fun profiles(): List<AgentProfile> = Agentd.readProfiles(profilesReply())

    // ---- projects --------------------------------------------------------

    @Test
    fun `every field of a project record survives the wire`() {
        val rows = projects()
        assertEquals(3, rows.size, "the fixture lost a row")

        val apex = rows.first { it.name == "apex-os" }
        assertEquals("/var/home/andre/Projects/apex/apex-os", apex.root)
        assertEquals("apex-os-1a2b3c4d", apex.slug)
        assertEquals(listOf("container", "kotlin", "rust"), apex.languages)
        // SECONDS, like `SessionInfo.started` and unlike every `_ms` field on
        // this socket. A reader that took it for milliseconds would date every
        // project to January 1970.
        assertEquals(1_758_499_200L, apex.lastOpened)
    }

    @Test
    fun `the capsule is the workspace, and an unbound project says so`() {
        // P1-056 asks to browse "workspaces". §8's capsule is the only object
        // in this runtime that is one, and the binding lives on the project —
        // so if this field did not survive the wire, that criterion would have
        // nothing behind it at all.
        val rows = projects()
        assertEquals("rust", rows.first { it.name == "apex-os" }.capsule)
        assertNull(
            rows.first { it.name == "apex-shell" }.capsule,
            "an unbound project must not borrow a neighbour's workspace",
        )
    }

    @Test
    fun `a project with no detected toolchain is not a project with no record`() {
        // An empty `languages` is a real answer — a repository with no marker
        // file this runtime knows — and it is not the same as a missing field.
        // A parser that treated the two alike would drop the row.
        val scratch = projects().first { it.name == "scratch" }
        assertTrue(scratch.languages.isEmpty())
        assertEquals("scratch-9c0d1e2f", scratch.slug)
        assertEquals(0L, scratch.lastOpened)
    }

    @Test
    fun `a project joins its sessions on the root and not on the name`() {
        // `SessionInfo.project` is the project ROOT verbatim — the daemon's
        // own field comment says so — while `project_name` is for display.
        // Joining on the name would put two checkouts of the same repository
        // in one another's rows, which on this machine is the normal case:
        // there are over a hundred worktrees of `apex-os` here.
        val apex = projects().first { it.name == "apex-os" }
        val sessions = listOf(
            session(id = 1, project = apex.root),
            session(id = 2, project = "/var/home/andre/Projects/apex/apex-shell"),
            session(id = 3, project = null),
        )
        assertEquals(listOf(1), apex.sessionsIn(sessions).map { it.id })
    }

    @Test
    fun `the listing is ordered most recently opened first`() {
        // The first row is a screen's default selection, so this is a property
        // the Start screen depends on rather than a cosmetic one.
        val ordered = ProjectRecord.ordered(projects().reversed())
        assertEquals(listOf("apex-os", "apex-shell", "scratch"), ordered.map { it.name })
    }

    @Test
    fun `an empty listing is an answer and not a failure`() {
        // A machine where nobody has opened a project yet. It must parse to an
        // empty list, because a screen that showed this as an error would
        // report a fresh machine as a broken one.
        assertEquals(emptyList<ProjectRecord>(), Agentd.readProjects("""{"reply":"projects","projects":[]}"""))
    }

    @Test
    fun `a refusal is raised as an error and never read as an empty machine`() {
        // The version-skew case. A machine older than this verb answers
        // `bad_request` with serde's "unknown variant", and a client that read
        // that as "no projects" would report a stale runtime as an empty one.
        val err = assertThrows(AgentError::class.java) {
            Agentd.readProjects(
                """{"reply":"error","kind":"bad_request",""" +
                    """"message":"unparseable request: unknown variant `projects`, expected one of ..."}""",
            )
        }
        assertTrue(Agentd.isTooOld(err), "the skew case must be distinguishable from a real refusal")
    }

    // ---- profiles --------------------------------------------------------

    @Test
    fun `every field of a profile row survives the wire`() {
        val rows = profiles()
        assertEquals(3, rows.size, "the fixture lost a row")

        val claude = rows.first { it.agent == "claude" }
        assertEquals("Claude Code", claude.display)
        assertEquals("claude", claude.program)
        assertTrue(claude.programFound)
        assertFalse(claude.commandRequired)
        assertTrue(claude.described)
        assertEquals("~/.claude", claude.root)
        assertTrue(claude.installed)
        assertEquals(5, claude.reusable)
        assertEquals(2, claude.mixed)
        assertEquals(6, claude.machineLocal)
        assertEquals(2, claude.secret)
    }

    @Test
    fun `an agent that is not on the machine says so before the tap`() {
        // The defect a device found, from the other side: the Start screen
        // offered an adapter whose only possible outcome was the daemon's
        // refusal after a round trip. The daemon resolves this on ITS PATH,
        // which is the one that matters and the one no client can derive.
        val codex = profiles().first { it.agent == "codex" }
        assertFalse(codex.programFound)
        assertFalse(codex.programIsPresent, "an absent program is not a startable agent")
        val caution = codex.caution
        assertNotNull(caution)
        assertTrue(caution!!.contains("PATH"), "the sentence must say what is missing: $caution")
        assertTrue(caution.contains("codex"), "and which program it is: $caution")
    }

    @Test
    fun `the adapter that runs anything is not reported as missing`() {
        // `generic` has no program of its own, so `program_found` is false and
        // means nothing. A screen that read that field alone would grey out
        // the one adapter that always works.
        val generic = profiles().first { it.agent == "generic" }
        assertTrue(generic.commandRequired)
        assertNull(generic.program)
        assertFalse(generic.programFound)
        assertTrue(generic.programIsPresent, "the adapter that runs anything was called missing")
        assertEquals("Runs whatever program you name.", generic.caution)
    }

    @Test
    fun `an installed program with no profile behind it is worth saying`() {
        // An agent can be on PATH with no profile directory: a startable
        // session with none of the user's instructions, skills or commands in
        // it. Saying so before the tap is the point of the `installed` field.
        val bare = AgentProfile(
            agent = "claude",
            display = "Claude Code",
            program = "claude",
            programFound = true,
            described = true,
            root = "~/.claude",
            installed = false,
        )
        val caution = bare.caution
        assertNotNull(caution)
        assertTrue(caution!!.contains("~/.claude"), caution)
        assertTrue(bare.programIsPresent, "the agent is there; its profile is not")

        // And a healthy row says nothing at all. A row that always carries a
        // sentence trains the reader to stop reading it, and then the one that
        // matters reads like the rest.
        assertNull(bare.copy(installed = true).caution)
    }

    @Test
    fun `the reply carries no profile content`() {
        // The audit, restated at the parser. The daemon has the configured
        // model, the plugins, the marketplaces, the MCP servers and the
        // credentials (`profile::doctor`) and sends none of it. If a field is
        // ever added that does, this fails on the fixture before it ships.
        val fields = AgentProfile.serializer().descriptor
        val names = (0 until fields.elementsCount).map { fields.getElementName(it) }.toSet()
        assertEquals(
            setOf(
                "agent", "display", "program", "program_found", "command_required",
                "described", "root", "installed",
                "reusable", "mixed", "machine_local", "secret",
            ),
            names,
            "a field was added to the profile row; check it carries a count and not content",
        )
        val text = profilesReply()
        for (leak in listOf("settings.json", "credentials", "mcpServers", "SKILL", "plugins")) {
            assertFalse(text.contains(leak), "the fixture carries $leak, which is content")
        }
    }

    @Test
    fun `an unknown adapter is rendered rather than dropped`() {
        // A daemon newer than this app lists an adapter this app has never
        // heard of. It must arrive as itself — a row with an id and a display
        // name — and not throw, because on a phone a throw here means the
        // whole picker disappears and the user cannot start anything at all.
        val rows = Agentd.readProfiles(
            """{"reply":"profiles","profiles":[""" +
                """{"agent":"newthing","display":"New Thing","program":"newthing",""" +
                """"program_found":true,"command_required":false,"described":false,""" +
                """"root":null,"installed":false,"reusable":0,"mixed":0,""" +
                """"machine_local":0,"secret":0,"a_field_from_the_future":7}]}""",
        )
        assertEquals(1, rows.size)
        assertEquals("newthing", rows[0].agent)
        assertTrue(rows[0].programIsPresent)
    }

    private fun session(id: Int, project: String?): AgentSession = AgentSession(
        id = id,
        agent = "claude",
        cwd = project ?: "/",
        project = project,
        state = "working",
    )
}
