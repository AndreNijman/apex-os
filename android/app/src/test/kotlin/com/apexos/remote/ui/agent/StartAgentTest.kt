package com.apexos.remote.ui.agent

import com.apexos.remote.core.agent.AgentSession
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

/**
 * The one part of the start screen that is a decision rather than a layout.
 *
 * P1-054 asks for "profile/project/worktree", and two of those three are not
 * things this socket can be asked about: `apex-agent-core`'s `Request` has no
 * profile verb, no projects verb and no worktrees verb. So the directories
 * offered are derived from what the daemon has already said, and that
 * derivation is testable where a picker is not.
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
        // And the screen says why: there is no verb to ask a daemon for its
        // projects. A phone that guessed `/home/<something>` would be offering
        // a path it invented.
        assertEquals(emptyList<String>(), knownDirectories(emptyList()))
    }
}
