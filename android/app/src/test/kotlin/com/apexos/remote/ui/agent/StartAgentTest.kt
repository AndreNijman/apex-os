package com.apexos.remote.ui.agent

import com.apexos.remote.core.agent.AgentProfile
import com.apexos.remote.core.agent.AgentSession
import com.apexos.remote.core.agent.ProjectRecord
import com.apexos.remote.ui.PickerUiState
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

/**
 * The parts of the start screen that are decisions rather than layout.
 *
 * P1-054 asks for "profile/project/worktree". The comment that stood here said
 * there was no profile verb and no projects verb, which was true when it was
 * written: both are on the wire now, and
 * [com.apexos.remote.core.agent.ProjectRecord] carries the account of how that
 * is asserted rather than remembered.
 *
 * What did not change is that every machine in the field predates them, so the
 * derived-directory fallback below stays and stays tested. What is new is that
 * the screen must now say WHICH of two reasons it is falling back for, and
 * must not let "this app could not ask" read as "the agent is not installed".
 */
class StartAgentTest {
    private fun session(id: Int, cwd: String, project: String? = null, lastActivity: Long = 0) =
        AgentSession(id = id, cwd = cwd, project = project, lastActivity = lastActivity)

    @Test
    fun `the directories offered are the ones the machine has already mentioned`() {
        val dirs = knownDirectories(
            listOf(
                session(1, cwd = "/home/a/one", lastActivity = 10),
                session(2, cwd = "/home/a/two", project = "/home/a", lastActivity = 20),
            ),
        )
        // Newest first, and a project before the directory inside it: the
        // project is the thing somebody means when they start a second agent.
        assertEquals(listOf("/home/a", "/home/a/two", "/home/a/one"), dirs)
    }

    @Test
    fun `a directory mentioned twice is offered once`() {
        val dirs = knownDirectories(
            listOf(
                session(1, cwd = "/home/a/p", lastActivity = 5),
                session(2, cwd = "/home/a/p", lastActivity = 9),
                session(3, cwd = "/home/a/p", project = "/home/a/p", lastActivity = 7),
            ),
        )
        assertEquals(listOf("/home/a/p"), dirs)
    }

    @Test
    fun `anything that is not an absolute path is not offered`() {
        // `RunRequest.cwd` must be absolute — the daemon says so — and
        // `MachineLink.run` refuses a relative one before it reaches the wire.
        // Offering one as a chip would be offering a button that cannot work.
        val dirs = knownDirectories(
            listOf(
                session(1, cwd = "", lastActivity = 3),
                session(2, cwd = "relative/path", lastActivity = 4),
                session(3, cwd = "/absolute", lastActivity = 5),
            ),
        )
        assertEquals(listOf("/absolute"), dirs)
        assertTrue(dirs.all { it.startsWith("/") })
        assertFalse(dirs.contains(""))
    }

    @Test
    fun `a machine with no sessions offers nothing rather than a guess`() {
        // A phone that guessed `/home/<something>` would be offering a path it
        // invented.
        assertEquals(emptyList<String>(), knownDirectories(emptyList()))
    }

    // ---- what the fallback says, and why it is four sentences -------------

    @Test
    fun `an old machine and an empty one are not told apart by an empty list`() {
        // The two facts that reach this screen as the same empty project list.
        // Telling the user the wrong one sends them looking in the wrong
        // place: one is fixed by updating the machine, the other by opening a
        // project on it.
        val old = directoryFallbackReason(PickerUiState(asked = true, tooOld = true))
        assertTrue(old.contains("predates"), old)

        val empty = directoryFallbackReason(PickerUiState(asked = true))
        assertTrue(empty.contains("no remembered projects"), empty)
        assertFalse(empty.contains("predates"), empty)

        // A refusal carries the daemon's own words rather than either of them.
        val refused = directoryFallbackReason(PickerUiState(asked = true, failure = "boom"))
        assertTrue(refused.contains("boom"), refused)

        // And before the answer arrives, neither claim is made.
        val pending = directoryFallbackReason(PickerUiState(loading = true))
        assertFalse(pending.contains("predates"), pending)
        assertFalse(pending.contains("no remembered projects"), pending)
    }

    @Test
    fun `the project line names the workspace, bound or not`() {
        // §8's capsule is the "workspace" P1-056 asks to browse, and it is the
        // only object in this runtime that is one. Stated either way: an
        // omitted line for an unbound project reads as "unknown", and the
        // difference matters when the next thing you do is start an agent in
        // it.
        val bound = ProjectRecord(
            root = "/home/a/apex",
            name = "apex",
            slug = "apex-1",
            languages = listOf("rust", "kotlin"),
            capsule = "rust",
        )
        val line = projectDetail(bound)
        assertTrue(line.contains("/home/a/apex"), line)
        assertTrue(line.contains("rust, kotlin"), line)
        assertTrue(line.contains("workspace rust"), line)

        val unbound = projectDetail(bound.copy(capsule = null, languages = emptyList()))
        assertTrue(unbound.contains("no workspace bound"), unbound)
        assertTrue(unbound.contains("no toolchain detected"), unbound)

        // Nothing chosen yet: an instruction, not a claim about any project.
        assertTrue(projectDetail(null).contains("Choose one"), projectDetail(null))
    }

    // ---- what Start is allowed to do -------------------------------------

    @Test
    fun `an agent the machine does not have cannot be started from here`() {
        // The defect a device found, closed at the button. The daemon resolves
        // the program on ITS PATH; before this the phone could not know, so
        // Start was enabled and its only possible outcome was a refusal after
        // a round trip.
        val absent = AgentProfile(agent = "codex", program = "codex", programFound = false)
        val present = AgentProfile(agent = "claude", program = "claude", programFound = true)
        val profiles = listOf(absent, present)

        assertFalse(startIsPossible("codex", "/home/a", "", profiles))
        assertTrue(startIsPossible("claude", "/home/a", "", profiles))
    }

    @Test
    fun `a machine that could not be asked does not disable anything`() {
        // The direction that must NOT fail closed. An empty profiles list
        // means "this app could not ask" — every machine in the field today —
        // and reading it as "the agent is not there" would grey out Start on
        // every one of them.
        assertTrue(startIsPossible("claude", "/home/a", ""))
        assertTrue(startIsPossible("codex", "/home/a", "", emptyList()))

        // And an adapter the listing does not mention is not assumed missing.
        assertTrue(
            startIsPossible("kimi", "/home/a", "", listOf(AgentProfile(agent = "claude", programFound = true))),
        )
    }

    @Test
    fun `the command rule follows the daemon's flag and falls back to the id`() {
        // With no listing, `generic` still needs a command: that machine still
        // branches on the id, so the id rule is the right fallback.
        assertFalse(startIsPossible("generic", "/home/a", ""))
        assertTrue(startIsPossible("generic", "/home/a", "/bin/cat"))

        // With a listing, the daemon's own flag decides — asserted with a row
        // that DISAGREES with the id rule, because rows that agree would pass
        // against an implementation that ignored the listing.
        val stated = listOf(
            AgentProfile(agent = "generic", commandRequired = false, programFound = true),
            AgentProfile(agent = "someagent", commandRequired = true, programFound = true),
        )
        assertTrue(startIsPossible("generic", "/home/a", "", stated))
        assertFalse(startIsPossible("someagent", "/home/a", "", stated))
        assertTrue(startIsPossible("someagent", "/home/a", "htop", stated))
    }

    @Test
    fun `a relative directory still stops Start whatever the profiles say`() {
        val profiles = listOf(AgentProfile(agent = "claude", programFound = true))
        assertFalse(startIsPossible("claude", "relative", "", profiles))
        assertFalse(startIsPossible("claude", "", "", profiles))
    }
}
