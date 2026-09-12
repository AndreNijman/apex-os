package com.apexos.remote.core.agent

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

/**
 * `apex-agentd`'s per-worktree status, as a phone reads it.
 *
 * ## The verb exists. The note this file replaces said it did not.
 *
 * `Sessions.kt` carried a long comment asserting that `apex-agent-core`'s
 * `Request` vocabulary "is Hello, Run, List, Info, Attach, Resize, Signal,
 * Event, Logs, Remove, Prune, and the privilege and secret verbs", and that a
 * `{"cmd":"worktrees"}` request would reach a daemon that cannot deserialise
 * it. That was measured against the wrong enum. `apex-agent-core` has **two**
 * types called after requests:
 *
 * * `request.rs`'s `Verb` — the *privileged operation* vocabulary (`install`,
 *   `remove`, `pkg-upgrade`, `pin`, `rollback`, `update`). No worktrees, no
 *   projects, no profile, correctly.
 * * `protocol.rs`'s `Request` — the *wire* vocabulary, `#[serde(tag = "cmd",
 *   rename_all = "snake_case")]`. It has carried
 *   `Worktrees { project: Option<String> }` since commit `473b7f60`
 *   (2026-09-08), it is dispatched at `apex-agentd/src/main.rs:906`, and
 *   `apex-remoted` forwards it: `serve.rs`'s `control()` is a denylist of
 *   exactly one verb, `attach`.
 *
 * So a phone may ask this question, and this file is the answer's parser.
 *
 * ## Two shapes that would fail silently
 *
 * * **`ConflictState` and `TestState` are internally tagged on `"state"`**,
 *   not on serde's default. `{"state":"conflicted","paths":[…]}`.
 * * **An unrecognised state must never read as the good one.** The daemon's
 *   own comment calls `Clean` "the one wrong answer here that costs somebody
 *   a broken merge", and its `status()` starts every worktree at `Unknown` so
 *   that a case nobody thought of cannot fall through to it. A sealed Kotlin
 *   hierarchy would *throw* on a variant a newer daemon added, which on a
 *   phone means the whole listing disappears; a `when` with an `else ->` that
 *   picked a default would be worse still. Both are carried as the tag STRING
 *   with the variant fields beside it, so a state this app has never heard of
 *   renders as itself and is neither clean nor passing.
 */
@Serializable
data class WorktreeStatus(
    /** The worktree's directory name. The main tree's is the project's. */
    val name: String = "",
    /**
     * The slug of the project this worktree belongs to.
     *
     * Empty against a daemon that predates the field — it is `#[serde(default)]`
     * on the Rust side — and an empty string is not a slug and cannot be
     * mistaken for one. [Project.group] falls back to the emission-order rule
     * when it is absent, so a listing from an older runtime groups exactly as
     * it did before.
     */
    val slug: String = "",
    val path: String = "",
    val branch: String? = null,
    val head: String? = null,
    /** False for the project's own main working tree — the group header. */
    @SerialName("is_agent") val isAgent: Boolean = false,
    /** The branch this one is measured against, observed at query time. */
    val base: String? = null,
    /** Uncommitted changes, staged or not, including untracked files. */
    val dirty: Boolean = false,
    val diff: DiffSummary = DiffSummary(),
    /** Commits this worktree has that [base] does not. */
    val ahead: Int? = null,
    /** Commits [base] has that this worktree does not. */
    val behind: Int? = null,
    val upstream: String? = null,
    /** Commits not yet pushed to [upstream]. */
    val unpushed: Int? = null,
    val conflicts: ConflictStatus = ConflictStatus(),
    val tests: TestStatus = TestStatus(),
    /** Sessions the daemon has working in this worktree. Ids only. */
    val sessions: List<Int> = emptyList(),
    val ready: Readiness = Readiness(),
) {
    /** Branch if it is on one, else the short head, else the directory name. */
    val label: String get() = branch ?: head?.take(12) ?: name

    /**
     * Whether anything at all is going on here. A clean idle worktree is not.
     *
     * A RUNNING test counts, and that is the case worth naming: a worktree
     * with nothing committed, nothing dirty and no session attached still has
     * a suite going in it, and a rule built only from git facts would draw it
     * as idle while it is the busiest tree in the project.
     */
    val hasWork: Boolean
        get() = dirty ||
            (ahead ?: 0) > 0 ||
            diff.files > 0 ||
            sessions.isNotEmpty() ||
            tests.isRunning
}

/** Files and lines changed. A **count** of files — the daemon sends no names. */
@Serializable
data class DiffSummary(
    val files: Int = 0,
    val insertions: Int = 0,
    val deletions: Int = 0,
) {
    val isEmpty: Boolean get() = files == 0 && insertions == 0 && deletions == 0
}

/**
 * Whether this worktree would merge back cleanly.
 *
 * Carried as the tag string rather than a sealed hierarchy: see the note on
 * [WorktreeStatus]. [isClean] is true for exactly one spelling and every other
 * answer — including one from a newer daemon — is not clean.
 */
@Serializable
data class ConflictStatus(
    val state: String = UNKNOWN,
    /** The conflicting paths. Present only on [CONFLICTED]. */
    val paths: List<String> = emptyList(),
    /** Why git declined to answer. Present only on [UNKNOWN]. */
    val reason: String? = null,
) {
    val isClean: Boolean get() = state == CLEAN
    val isConflicted: Boolean get() = state == CONFLICTED
    val isApplicable: Boolean get() = state != NOT_APPLICABLE

    companion object {
        const val CLEAN = "clean"
        const val CONFLICTED = "conflicted"
        const val UNKNOWN = "unknown"
        const val NOT_APPLICABLE = "not_applicable"
    }
}

/**
 * The last test run APEX observed in a worktree.
 *
 * Deliberately not a boolean, for the daemon's own reason: "never saw one",
 * "one is running" and "the last one failed" are three different things to a
 * person deciding whether to hand work over.
 *
 * `started` is set on [RUNNING] and `finished` on [PASSED]/[FAILED]; both are
 * unix **seconds**, matching `SessionInfo.started`.
 */
@Serializable
data class TestStatus(
    val state: String = UNOBSERVED,
    val command: String? = null,
    val started: Long? = null,
    val finished: Long? = null,
    /** The commit the worktree was on when the run started. */
    val head: String? = null,
) {
    val isFailed: Boolean get() = state == FAILED
    val isPassed: Boolean get() = state == PASSED
    val isRunning: Boolean get() = state == RUNNING

    /** When this was observed, unix seconds, or 0 for [UNOBSERVED]. */
    val whenSeconds: Long get() = finished ?: started ?: 0L

    companion object {
        const val UNOBSERVED = "unobserved"
        const val RUNNING = "running"
        const val PASSED = "passed"
        const val FAILED = "failed"
    }
}

/**
 * Whether the work in a worktree is ready to be handed to a human.
 *
 * `readyToPropose`, **not** `prStatus`, and the daemon's name is load-bearing:
 * nothing here asks GitHub anything. It is a local judgement from local facts,
 * and a field called `pr_status` would be read as a live query against a forge.
 * A phone must not label it "PR ready" either.
 */
@Serializable
data class Readiness(
    @SerialName("ready_to_propose") val readyToPropose: Boolean = false,
    /** Why not, in the order a person would fix them. Empty when ready. */
    val blockers: List<String> = emptyList(),
)

/**
 * The worktrees of one project, as a phone groups them.
 *
 * ## Why grouping is derived and not read off a field
 *
 * `Response::Worktrees` is a **flat** list: `WorktreeStatus` carries no
 * project slug, and the daemon's own doc says the rows arrive "main tree
 * first, projects in listing order" — `worktrees.rs` appends
 * `worktree::statuses(proj, …)` per remembered project, and `statuses` emits
 * that project's main tree first.
 *
 * So a row with `is_agent == false` begins a project, and every `is_agent`
 * row after it belongs to that project until the next one. That is a
 * derivation from an emission order rather than from a stated fact, which is
 * why [group] does not *assume* it holds: a listing that begins with agent
 * rows, or one for a project whose main tree could not be statted, has no
 * header to hang them on, and those rows go into an [Project.UNGROUPED]
 * project rather than being silently attributed to the wrong one or dropped.
 */
@Serializable
data class Project(
    /** The main working tree, when the listing had one. */
    val main: WorktreeStatus?,
    /** Its agent worktrees, in the order the daemon listed them. */
    val worktrees: List<WorktreeStatus>,
) {
    /** The project's own name: its main tree's, or a placeholder. */
    val name: String get() = main?.name ?: UNGROUPED

    /** The project slug, when the daemon was new enough to state it. */
    val slug: String get() = main?.slug ?: worktrees.firstOrNull()?.slug ?: ""

    val path: String? get() = main?.path

    /** Every row, main tree first — what a flat list for this project shows. */
    val rows: List<WorktreeStatus>
        get() = if (main == null) worktrees else listOf(main) + worktrees

    /** Sessions working anywhere in this project. */
    val sessions: List<Int> get() = rows.flatMap { it.sessions }

    val anyConflicted: Boolean get() = rows.any { it.conflicts.isConflicted }
    val anyFailing: Boolean get() = rows.any { it.tests.isFailed }

    companion object {
        /**
         * The name given to rows that arrived with no main tree before them.
         *
         * Not a project the user has: a label for rows whose project could not
         * be determined, shown as such rather than folded into a real one.
         */
        const val UNGROUPED = "(worktrees with no project row)"

        /**
         * Group a flat `worktrees` reply, preserving the daemon's order.
         *
         * Every input row appears in exactly one output project and none is
         * dropped — a listing that loses a worktree is worse than one that
         * groups it oddly, because the user cannot see that it is missing.
         */
        fun group(rows: List<WorktreeStatus>): List<Project> {
            // The daemon states the grouping when it is new enough to
            // (`WorktreeStatus.slug`), and then no inference is needed. Rows
            // keep their arrival order within a project and projects keep
            // theirs, because that order is `project::list()`'s and a screen
            // that re-sorted would disagree with `apex project list`.
            if (rows.any { it.slug.isNotEmpty() }) {
                val bySlug = LinkedHashMap<String, MutableList<WorktreeStatus>>()
                for (row in rows) bySlug.getOrPut(row.slug) { mutableListOf() } += row
                return bySlug.map { (_, group) ->
                    Project(
                        main = group.firstOrNull { !it.isAgent },
                        // Everything that is not the main tree, including a
                        // SECOND non-agent row should one ever appear: losing
                        // a worktree is worse than an odd grouping.
                        worktrees = group.filterIndexed { i, w ->
                            w.isAgent || i != group.indexOfFirst { !it.isAgent }
                        },
                    )
                }
            }

            // A daemon older than the slug field. Rows arrive main-tree-first
            // per project, so a non-agent row opens a project — a derivation
            // from an emission order rather than from a stated fact, which is
            // why rows with no main tree before them are not attributed to
            // whatever project comes next.
            val out = mutableListOf<Project>()
            var main: WorktreeStatus? = null
            var kids = mutableListOf<WorktreeStatus>()
            var started = false
            for (row in rows) {
                if (!row.isAgent) {
                    if (started) out += Project(main, kids)
                    main = row
                    kids = mutableListOf()
                    started = true
                } else {
                    kids += row
                    started = true
                }
            }
            if (started) out += Project(main, kids)
            return out
        }
    }
}
