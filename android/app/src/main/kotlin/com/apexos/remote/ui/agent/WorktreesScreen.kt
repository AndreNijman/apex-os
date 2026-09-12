package com.apexos.remote.ui.agent

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.apexos.remote.core.agent.ConflictStatus
import com.apexos.remote.core.agent.Project
import com.apexos.remote.core.agent.TestStatus
import com.apexos.remote.core.agent.WorktreeStatus
import com.apexos.remote.ui.WorktreesUiState
import com.apexos.remote.ui.theme.ApexTones
import com.apexos.remote.ui.theme.MachineText

/**
 * Projects, their worktrees, and what is going on in each (P1-056).
 *
 * ## Three answers that are not each other
 *
 * "Nothing came back" has three causes and this screen draws them as three
 * things, because folding them together is the specific failure this project
 * names as "permission denied is not absence":
 *
 * * **The machine has no projects.** An empty list, said as such.
 * * **The machine's APEX predates the verb.** `WorktreesUiState.tooOld`. The
 *   daemon answers `bad_request` with "unknown variant `worktrees`", which is
 *   the same error kind it uses for a refusal — `Agentd.isTooOld` tells them
 *   apart on the message. Drawn as an empty list this would be reporting a
 *   version skew as a fact about the user's machine, and it is not
 *   hypothetical: the image on the developer's own laptop is from 2026-09-05
 *   and the verb landed 2026-09-08, so this is the ONLY answer obtainable
 *   here.
 * * **The request failed.** Shown with the daemon's words.
 *
 * ## Why there is a button and not a poll
 *
 * Answering `worktrees` makes the daemon run git in EVERY remembered project,
 * `merge-tree --write-tree` included. On the Agent Center's four-second timer
 * that would be a phone in a pocket driving git across a laptop's disk all
 * day. It is asked when the screen opens and when the user asks again.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun WorktreesScreen(
    machine: String,
    state: WorktreesUiState,
    /** Session ids this machine currently has, so a row knows if it can open one. */
    liveSessions: Set<Int>,
    onLoad: () -> Unit,
    onOpenSession: (Int) -> Unit,
    onBack: () -> Unit,
) {
    // Asked on arrival rather than left behind a button nobody would think to
    // press on an empty screen. Keyed on the machine so that coming back to a
    // different one asks again.
    LaunchedEffect(machine) { if (!state.everAsked) onLoad() }

    Scaffold(
        containerColor = MaterialTheme.colorScheme.background,
        topBar = {
            TopAppBar(
                title = { Text("Projects", style = MaterialTheme.typography.titleMedium) },
                navigationIcon = { TextButton(onClick = onBack) { Text("Back") } },
                actions = {
                    TextButton(onClick = onLoad, enabled = !state.loading) {
                        Text(if (state.loading) "Asking…" else "Refresh")
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(
                    containerColor = MaterialTheme.colorScheme.background,
                ),
            )
        },
    ) { padding ->
        Column(Modifier.fillMaxSize().padding(padding)) {
            if (state.loading) {
                Strip("Running git in every remembered project. This is slower than the rest of the app.", alarming = false)
            }
            state.failure?.let { Strip(it, alarming = true) }

            when {
                state.tooOld -> TooOld(onLoad)
                state.projects.isEmpty() && !state.loading && state.failure == null && state.everAsked ->
                    NoProjects()
                else -> LazyColumn(Modifier.weight(1f)) {
                    for (project in state.projects) {
                        item(key = "p" + project.slug + project.name) {
                            ProjectHeading(project)
                        }
                        for (row in project.worktrees) {
                            item(key = "w" + row.path) {
                                WorktreeRow(row, liveSessions, onOpenSession)
                            }
                        }
                        if (project.worktrees.isEmpty()) {
                            item(key = "e" + project.name) { NoWorktrees() }
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun ProjectHeading(project: Project) {
    Column(Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 18.dp, bottom = 6.dp)) {
        Text(project.name, style = MaterialTheme.typography.titleSmall)
        project.path?.let {
            Text(
                it,
                style = MachineText,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.MiddleEllipsis,
            )
        }
        // The project's main tree, summarised on the heading rather than given
        // a row of its own: it is the thing the worktrees are measured
        // against, not one of them.
        project.main?.let { main ->
            val parts = buildList {
                main.branch?.let { add(it) }
                if (main.dirty) add("uncommitted changes")
                if (project.anyConflicted) add("a worktree will not merge cleanly")
                if (project.anyFailing) add("a test run failed")
            }
            if (parts.isNotEmpty()) {
                Text(
                    parts.joinToString(" · "),
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
        if (project.main == null) {
            // `Project.UNGROUPED`: rows the daemon emitted with no main tree
            // before them. Said outright rather than folded into a real
            // project, because attributing a worktree to the wrong project is
            // worse than admitting the grouping is unknown.
            Text(
                "APEX listed these worktrees without a project row before them, so which " +
                    "project they belong to is not stated. They are shown here rather than " +
                    "guessed at or dropped.",
                style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
    HorizontalDivider(color = MaterialTheme.colorScheme.surfaceVariant)
}

/**
 * One worktree.
 *
 * The row is tappable only when the daemon says a session is working in it AND
 * this phone still has that session. The join from a worktree to a session is
 * the part a client running git itself could not compute, so it is followed
 * from `WorktreeStatus.sessions` and never guessed from a path.
 */
@Composable
private fun WorktreeRow(
    row: WorktreeStatus,
    liveSessions: Set<Int>,
    onOpenSession: (Int) -> Unit,
) {
    val openable = row.sessions.firstOrNull { it in liveSessions }
    val tone = when {
        row.conflicts.isConflicted -> ApexTones.forState("failed")
        row.tests.isFailed -> ApexTones.forState("failed")
        row.tests.isRunning -> ApexTones.forState("working")
        row.ready.readyToPropose -> ApexTones.forState("complete")
        row.hasWork -> ApexTones.forState("working")
        else -> MaterialTheme.colorScheme.surfaceVariant
    }
    Row(
        Modifier
            .fillMaxWidth()
            .then(if (openable != null) Modifier.clickable { onOpenSession(openable) } else Modifier)
            .padding(end = 16.dp, top = 10.dp, bottom = 10.dp),
        verticalAlignment = Alignment.Top,
    ) {
        Box(Modifier.width(3.dp).height(56.dp).background(tone))
        Spacer(Modifier.width(13.dp))
        Column(Modifier.weight(1f)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(row.label, style = MaterialTheme.typography.titleSmall, maxLines = 1)
                Spacer(Modifier.weight(1f))
                if (openable != null) {
                    Text(
                        "open agent",
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.primary,
                    )
                } else if (row.sessions.isNotEmpty()) {
                    // The daemon knows a session here; this phone does not
                    // have it. Said rather than shown as an inert row.
                    Text(
                        "an agent is here",
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
            Text(
                row.path,
                style = MachineText,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.MiddleEllipsis,
            )
            Changes(row)
            Conflicts(row.conflicts)
            Tests(row.tests)
            Readiness(row)
        }
    }
    HorizontalDivider(color = MaterialTheme.colorScheme.surfaceVariant)
}

/**
 * What changed, in the words the daemon's numbers support.
 *
 * `DiffSummary.files` is a **count**. The daemon sends no file names, so this
 * says "4 files" and never lists them — P1-056 asks to "view changed files",
 * and the honest form of that against this wire is the count plus the line
 * totals. A screen that offered a file list would have to invent one.
 */
@Composable
private fun Changes(row: WorktreeStatus) {
    val parts = buildList {
        if (!row.diff.isEmpty) {
            add("${row.diff.files} ${if (row.diff.files == 1) "file" else "files"}")
            add("+${row.diff.insertions} −${row.diff.deletions}")
        }
        if (row.dirty) add("uncommitted")
        row.ahead?.takeIf { it > 0 }?.let { add("$it ahead of ${row.base ?: "the base"}") }
        row.behind?.takeIf { it > 0 }?.let { add("$it behind") }
        row.unpushed?.takeIf { it > 0 }?.let { add("$it unpushed") }
    }
    if (parts.isEmpty()) {
        Text(
            "no changes",
            style = MaterialTheme.typography.labelSmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    } else {
        Text(parts.joinToString(" · "), style = MaterialTheme.typography.labelSmall)
    }
}

/**
 * Whether it would merge back.
 *
 * Four states and four sentences, and the fourth matters: a state this app has
 * never heard of renders as itself and is NOT drawn as clean. The daemon's own
 * `status()` starts every worktree at `Unknown` because `Clean` is, in its
 * words, "the one wrong answer here that costs somebody a broken merge".
 */
@Composable
private fun Conflicts(conflicts: ConflictStatus) {
    if (!conflicts.isApplicable) return
    val text = when (conflicts.state) {
        ConflictStatus.CLEAN -> "merges cleanly"
        ConflictStatus.CONFLICTED -> {
            val n = conflicts.paths.size
            if (n == 0) {
                "will not merge cleanly"
            } else {
                "will not merge cleanly — ${conflicts.paths.take(3).joinToString(", ")}" +
                    if (n > 3) " and ${n - 3} more" else ""
            }
        }
        ConflictStatus.UNKNOWN ->
            "git could not say whether this merges" +
                (conflicts.reason?.let { ": $it" } ?: "")
        // A state a newer APEX added. Named, not guessed at, and specifically
        // not shown as clean.
        else -> "merge state \"${conflicts.state}\", which this app does not know"
    }
    Text(
        text,
        style = MaterialTheme.typography.labelSmall,
        color = if (conflicts.isClean) {
            MaterialTheme.colorScheme.onSurfaceVariant
        } else {
            MaterialTheme.colorScheme.error
        },
        maxLines = 2,
        overflow = TextOverflow.Ellipsis,
    )
}

/**
 * The last test run APEX watched.
 *
 * "Never saw one" is not "passed" and not "failed", and the daemon keeps the
 * three apart deliberately. It also forgets them on a restart, which is the
 * honest behaviour and is why `unobserved` says nobody here saw one rather
 * than "no tests".
 */
@Composable
private fun Tests(tests: TestStatus) {
    if (tests.state == TestStatus.UNOBSERVED) return
    val text = when (tests.state) {
        TestStatus.RUNNING -> "tests running" + (tests.command?.let { ": $it" } ?: "")
        TestStatus.PASSED -> "tests passed" + (tests.command?.let { ": $it" } ?: "")
        TestStatus.FAILED -> "tests failed" + (tests.command?.let { ": $it" } ?: "")
        else -> "test state \"${tests.state}\", which this app does not know"
    }
    Text(
        text,
        style = MaterialTheme.typography.labelSmall,
        color = if (tests.isFailed) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
        maxLines = 1,
        overflow = TextOverflow.Ellipsis,
    )
}

/**
 * Whether the work is ready to hand to a human.
 *
 * **"Ready to propose", never "PR ready".** The daemon's field is
 * `ready_to_propose` and its own doc says the name is load-bearing: nothing
 * asks GitHub anything, it is a local judgement from local facts, and a label
 * saying "PR" would be read as a live query against a forge.
 */
@Composable
private fun Readiness(row: WorktreeStatus) {
    if (row.ready.readyToPropose) {
        Text(
            "ready to propose — APEX's judgement from what is on this machine, not from GitHub",
            style = MaterialTheme.typography.labelSmall,
            color = ApexTones.forState("complete"),
            maxLines = 2,
        )
    } else if (row.ready.blockers.isNotEmpty() && row.hasWork) {
        // Only when there is work. Every idle worktree is "not ready", and
        // saying so on each of them is noise.
        Text(
            "not ready: " + row.ready.blockers.joinToString("; "),
            style = MaterialTheme.typography.labelSmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

@Composable
private fun TooOld(onRetry: () -> Unit) {
    Column(
        Modifier.fillMaxSize().padding(32.dp),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text("This machine's APEX is too old", style = MaterialTheme.typography.titleMedium)
        Spacer(Modifier.height(8.dp))
        Text(
            "The machine answered that it does not know the `worktrees` request. That is a " +
                "version difference, not an empty list and not a refusal — it has projects, " +
                "this build of APEX just cannot be asked about them. Run `sudo apex update` " +
                "on the machine.",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Spacer(Modifier.height(20.dp))
        Button(onClick = onRetry, shape = RoundedCornerShape(6.dp)) { Text("Ask again") }
    }
}

@Composable
private fun NoProjects() {
    Column(
        Modifier.fillMaxSize().padding(32.dp),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text("No projects", style = MaterialTheme.typography.titleMedium)
        Spacer(Modifier.height(8.dp))
        Text(
            "APEX remembers a project the first time an agent runs in it. Nothing on this " +
                "machine has been remembered yet.",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

@Composable
private fun NoWorktrees() {
    Text(
        "no agent worktrees",
        style = MaterialTheme.typography.labelSmall,
        fontFamily = FontFamily.Default,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(start = 16.dp, top = 8.dp, bottom = 12.dp),
    )
}
