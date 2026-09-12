package com.apexos.remote.core.agent

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertThrows
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

/**
 * The phone reads a real `worktrees` reply.
 *
 * `worktrees-reply.json` is the SHARED fixture: `apex-agent-core`'s
 * `tests/android_worktrees_wire.rs` deserializes the same file into its own
 * `Response` and asserts that re-serializing it produces the identical JSON
 * value. So this suite passing means the phone parses what the daemon sends,
 * rather than what somebody wrote down here. One of the two halves alone
 * proves nothing: a renamed field would leave a Kotlin-only test green against
 * a fixture nothing had checked.
 */
class WorktreesTest {
    private fun reply(): String =
        WorktreesTest::class.java.getResourceAsStream("/worktrees-reply.json")
            ?.bufferedReader()?.readText()
            ?: error("worktrees-reply.json is missing from core/src/test/resources/")

    private fun rows(): List<WorktreeStatus> = Agentd.readWorktrees(reply())

    // ---- the request ----------------------------------------------------

    @Test
    fun `the request omits the slug rather than sending a null`() {
        // `Request::Worktrees.project` is `Option<String>` with
        // `#[serde(default)]`, so both forms parse — but every other builder
        // in Agentd omits rather than nulls, and a listing request with a
        // literal null in it reads as a client that meant to send something.
        assertEquals("""{"cmd":"worktrees"}""", Agentd.worktrees())
        assertEquals("""{"cmd":"worktrees"}""", Agentd.worktrees(""))
    }

    @Test
    fun `a slug is escaped into the request`() {
        assertEquals("""{"cmd":"worktrees","project":"apex-os"}""", Agentd.worktrees("apex-os"))
        // A control character in a slug is ESCAPED rather than passed through.
        // An unescaped BEL would not fail here, it would fail at the far end:
        // `apex-remote-core`'s wire refuses a control payload containing a raw
        // control character in both directions, and the connection dies.
        assertEquals(
            """{"cmd":"worktrees","project":"a\u0007b"}""",
            Agentd.worktrees("a\u0007b"),
        )
    }

    @Test
    fun `the request never contains a newline`() {
        assertFalse(Agentd.worktrees("one\ntwo").contains('\n'))
    }

    // ---- the reply ------------------------------------------------------

    @Test
    fun `every row of a real reply is read`() {
        assertEquals(5, rows().size)
    }

    @Test
    fun `the scalar fields of an agent worktree survive the parse`() {
        val w = rows().first { it.name == "wt-p1-053b" }
        assertEquals("/var/home/andre/Projects/apex/apex-os/.apex/worktrees/wt-p1-053b", w.path)
        assertEquals("task/p1-053b-android-ux", w.branch)
        assertEquals("9c1e77aa0b31", w.head)
        assertTrue(w.isAgent)
        assertEquals("roadmap/v2.2", w.base)
        assertFalse(w.dirty)
        assertEquals(3, w.ahead)
        assertEquals(0, w.behind)
        assertEquals("origin/task/p1-053b-android-ux", w.upstream)
        assertEquals(0, w.unpushed)
        assertEquals(listOf(4, 11), w.sessions)
    }

    @Test
    fun `is_agent is read from the snake_case key the daemon sends`() {
        // Kotlin would default it to false in silence if the @SerialName were
        // wrong, and every row would then read as a project main tree — which
        // is precisely the field Project.group keys on.
        assertEquals(
            listOf(false, true, true, true, false),
            rows().map { it.isAgent },
        )
    }

    @Test
    fun `the diff summary is counts and the daemon sends no file names`() {
        val w = rows().first { it.name == "wt-p1-053b" }
        assertEquals(7, w.diff.files)
        assertEquals(412, w.diff.insertions)
        assertEquals(38, w.diff.deletions)
        assertFalse(w.diff.isEmpty)
        assertTrue(rows().first { it.name == "apex-shell" }.diff.isEmpty)
    }

    @Test
    fun `readiness is read with its blockers in order`() {
        val ready = rows().first { it.name == "wt-p1-053b" }.ready
        assertTrue(ready.readyToPropose)
        assertTrue(ready.blockers.isEmpty())

        val blocked = rows().first { it.name == "wt-conflicted" }.ready
        assertFalse(blocked.readyToPropose)
        assertEquals(
            listOf(
                "uncommitted changes",
                "would conflict with roadmap/v2.2",
                "the last test run failed",
            ),
            blocked.blockers,
        )
    }

    @Test
    fun `ready_to_propose is read from its own key and not defaulted`() {
        // The failure this guards is silent and one-directional: a wrong
        // @SerialName defaults it to false, every worktree reads as not ready,
        // and the screen looks merely pessimistic rather than broken.
        assertEquals(
            listOf(false, true, false, false, false),
            rows().map { it.ready.readyToPropose },
        )
    }

    // ---- the two internally-tagged enums --------------------------------

    @Test
    fun `all four conflict states are read from the state tag`() {
        assertEquals(
            listOf("not_applicable", "clean", "conflicted", "unknown", "not_applicable"),
            rows().map { it.conflicts.state },
        )
        val c = rows().first { it.name == "wt-conflicted" }.conflicts
        assertTrue(c.isConflicted)
        assertEquals(
            listOf("apexd/apex-agent-core/src/protocol.rs", "android/core/build.gradle.kts"),
            c.paths,
        )
        val u = rows().first { it.name == "wt-unknown" }.conflicts
        assertEquals("this worktree is not on a branch", u.reason)
        assertFalse(u.isClean)
    }

    @Test
    fun `all four test states are read from the state tag`() {
        assertEquals(
            listOf("unobserved", "passed", "failed", "running", "unobserved"),
            rows().map { it.tests.state },
        )
        val passed = rows().first { it.name == "wt-p1-053b" }.tests
        assertTrue(passed.isPassed)
        assertEquals("cargo test --locked", passed.command)
        assertEquals(1757650000L, passed.finished)
        assertNull(passed.started)
        assertEquals(1757650000L, passed.whenSeconds)

        val running = rows().first { it.name == "wt-unknown" }.tests
        assertTrue(running.isRunning)
        assertEquals(1757655000L, running.started)
        assertNull(running.finished)
        assertNull(running.head)
        assertEquals(1757655000L, running.whenSeconds)
    }

    @Test
    fun `a state this app has never heard of is neither clean nor passing`() {
        // The daemon's own comment calls Clean "the one wrong answer here that
        // costs somebody a broken merge", and `status()` starts every worktree
        // at Unknown so nothing can fall through to it. A sealed Kotlin
        // hierarchy would instead THROW on a variant a newer daemon added,
        // losing the whole listing. Neither happens: the tag is carried as
        // itself.
        val newer = """
            {"reply":"worktrees","worktrees":[{
              "name":"n","path":"/p","branch":null,"head":null,"is_agent":true,
              "base":null,"dirty":false,
              "diff":{"files":0,"insertions":0,"deletions":0},
              "ahead":null,"behind":null,"upstream":null,"unpushed":null,
              "conflicts":{"state":"rebase_in_progress","paths":[]},
              "tests":{"state":"flaky","command":"x","finished":1},
              "sessions":[],"ready":{"ready_to_propose":false,"blockers":[]}
            }]}
        """.trimIndent()
        val w = Agentd.readWorktrees(newer).single()
        assertEquals("rebase_in_progress", w.conflicts.state)
        assertFalse(w.conflicts.isClean)
        assertFalse(w.conflicts.isConflicted)
        assertEquals("flaky", w.tests.state)
        assertFalse(w.tests.isPassed)
        assertFalse(w.tests.isFailed)
    }

    @Test
    fun `a field a newer daemon adds does not lose the row`() {
        val newer = """
            {"reply":"worktrees","worktrees":[{
              "name":"n","path":"/p","is_agent":true,
              "diff":{"files":1,"insertions":1,"deletions":1,"binary":3},
              "conflicts":{"state":"clean"},"tests":{"state":"unobserved"},
              "sessions":[1],"ready":{"ready_to_propose":true,"blockers":[]},
              "stacked_on":"main"
            }]}
        """.trimIndent()
        assertEquals("n", Agentd.readWorktrees(newer).single().name)
    }

    // ---- errors ---------------------------------------------------------

    @Test
    fun `an error reply throws rather than reading as an empty machine`() {
        val e = assertThrows(AgentError::class.java) {
            Agentd.readWorktrees(
                """{"reply":"error","kind":"bad_request","message":"no remembered project with slug "x""}"""
                    .replace("\"x\"", "x"),
            )
        }
        assertEquals("bad_request", e.kind)
    }

    @Test
    fun `a runtime older than the verb is told apart from a runtime that refused`() {
        // Both are `bad_request`. Only one of them means "this machine's APEX
        // predates the feature", and a screen that showed the other as a
        // version skew would be telling the user to update for a typo.
        //
        // This is not hypothetical: the verb landed 2026-09-08 and the image
        // on the developer's own machine is from 2026-09-05.
        val old = AgentError(
            "bad_request",
            "unparseable request: unknown variant `worktrees`, expected one of `hello`, `run`, " +
                "`list` at line 1 column 20",
        )
        assertTrue(Agentd.isTooOld(old))

        val refused = AgentError(
            "bad_request",
            "no remembered project with slug \"nope\"; `apex project list` shows the ones there are",
        )
        assertFalse(Agentd.isTooOld(refused))

        val linked = AgentError(
            "bad_request",
            "\"wt\" is a linked git worktree, not a project — its worktrees are the repository's",
        )
        assertFalse(Agentd.isTooOld(linked))

        // A different kind is never a version skew, whatever it says.
        assertFalse(
            Agentd.isTooOld(AgentError("internal", "unparseable request: unknown variant `x`")),
        )
    }

    // ---- grouping -------------------------------------------------------

    @Test
    fun `a flat reply groups into the projects the daemon walked`() {
        val projects = Project.group(rows())
        assertEquals(listOf("apex-os", "apex-shell"), projects.map { it.name })
        assertEquals(
            listOf("wt-p1-053b", "wt-conflicted", "wt-unknown"),
            projects[0].worktrees.map { it.name },
        )
        assertTrue(projects[1].worktrees.isEmpty())
        assertEquals(listOf(4, 11, 7), projects[0].sessions)
        assertTrue(projects[0].anyConflicted)
        assertTrue(projects[0].anyFailing)
        assertFalse(projects[1].anyConflicted)
    }

    @Test
    fun `grouping never loses a row`() {
        // The property that matters more than getting the grouping right: a
        // user cannot see a worktree that is not on the screen.
        val all = rows()
        assertEquals(all.size, Project.group(all).sumOf { it.rows.size })
        assertEquals(all.map { it.name }.toSet(), Project.group(all).flatMap { p -> p.rows.map { it.name } }.toSet())
    }

    @Test
    fun `agent rows with no project row before them are shown as ungrouped`() {
        // `Response::Worktrees` carries no project slug, so the grouping is
        // derived from the daemon's emission order (main tree first, per
        // project). A listing that does not start with a main tree — a project
        // whose own tree could not be statted — must not have its worktrees
        // attributed to whatever project comes next.
        val orphans = rows().filter { it.isAgent }
        val projects = Project.group(orphans)
        assertEquals(1, projects.size)
        assertNull(projects[0].main)
        assertEquals(Project.UNGROUPED, projects[0].name)
        assertEquals(3, projects[0].worktrees.size)
    }

    @Test
    fun `an empty reply groups into nothing rather than one empty project`() {
        assertTrue(Project.group(emptyList()).isEmpty())
        assertTrue(Agentd.readWorktrees("""{"reply":"worktrees","worktrees":[]}""").isEmpty())
    }

    @Test
    fun `a project main tree is not counted as one of its own worktrees`() {
        val projects = Project.group(rows())
        assertEquals("apex-os", projects[0].main?.name)
        assertFalse(projects[0].worktrees.any { it.name == "apex-os" })
        assertEquals(4, projects[0].rows.size)
    }

    @Test
    fun `hasWork is false only for a worktree with nothing to show`() {
        val quiet = rows().first { it.name == "apex-shell" }
        assertFalse(quiet.hasWork)
        assertTrue(rows().first { it.name == "apex-os" }.hasWork) // dirty
        assertTrue(rows().first { it.name == "wt-p1-053b" }.hasWork) // ahead + sessions
        assertTrue(rows().first { it.name == "wt-unknown" }.hasWork) // a test is running
    }

    @Test
    fun `a worktree with no branch labels itself by its head and then its name`() {
        assertEquals("task/p1-053b-android-ux", rows().first { it.name == "wt-p1-053b" }.label)
        // No branch and no head: the directory name is all there is.
        assertEquals("wt-unknown", rows().first { it.name == "wt-unknown" }.label)
    }
}
