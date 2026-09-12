package com.apexos.remote.ui

import com.apexos.remote.core.agent.Project
import com.apexos.remote.core.agent.WorktreeStatus
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

/**
 * The three answers the Projects screen must keep apart.
 *
 * "No projects", "this machine's APEX is too old" and "the request failed" all
 * arrive as a listing that is not there, and drawing them the same way is the
 * failure this project names as "permission denied is not absence". The state
 * is where that is decided, so it is where it is tested.
 */
class WorktreesUiStateTest {

    private fun row(name: String, agent: Boolean, slug: String = "") =
        WorktreeStatus(name = name, slug = slug, path = "/p/$name", isAgent = agent)

    @Test
    fun `never asked is not the same as asked and empty`() {
        // The screen shows "No projects" only once a question has actually
        // been answered. Before that the answer is unknown, and an empty state
        // claiming the machine has no projects would be inventing a fact.
        assertFalse(WorktreesUiState().everAsked)
        assertTrue(WorktreesUiState(askedSeconds = 1).everAsked)
    }

    @Test
    fun `loading counts as asked, so the empty state does not flash first`() {
        // Without this the screen renders "No projects" for the whole time the
        // daemon spends running git in every project — which on a large
        // machine is seconds, and reads as a definite answer.
        assertTrue(WorktreesUiState(loading = true).everAsked)
    }

    @Test
    fun `too old carries no failure, because it is not one`() {
        // `Agentd.isTooOld` puts a version skew here rather than in `failure`,
        // so the screen can say "run apex update" instead of showing the
        // daemon's parser error to somebody who cannot act on it.
        val state = WorktreesUiState(tooOld = true, askedSeconds = 1)
        assertTrue(state.tooOld)
        assertEquals(null, state.failure)
        assertTrue(state.projects.isEmpty(), "and it is emphatically not a listing")
    }

    @Test
    fun `rows are every worktree of every project, main trees included`() {
        // This is what the alert watcher is handed, so a row missing here is a
        // test failure that never raises a notification.
        val projects = Project.group(
            listOf(
                row("apex", agent = false),
                row("wt-a", agent = true),
                row("wt-b", agent = true),
                row("other", agent = false),
                row("wt-c", agent = true),
            ),
        )
        val state = WorktreesUiState(projects = projects)
        assertEquals(5, state.rows.size, "a worktree was dropped between the reply and the screen")
        assertEquals(
            listOf("apex", "wt-a", "wt-b", "other", "wt-c"),
            state.rows.map { it.name },
            "the daemon's order is `project::list()`'s and must survive grouping",
        )
    }

    @Test
    fun `worktrees that arrived with no project row are still in rows`() {
        // `Project.UNGROUPED`. A row the screen cannot attribute is shown as
        // unattributed — a user cannot see a worktree that is not on the
        // screen, so losing one is worse than grouping it oddly.
        val state = WorktreesUiState(
            projects = Project.group(listOf(row("orphan", agent = true), row("apex", agent = false))),
        )
        assertEquals(2, state.rows.size)
        assertTrue(state.rows.any { it.name == "orphan" })
    }
}
