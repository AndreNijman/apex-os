package com.apexos.remote.core.agent

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

/**
 * A project the runtime remembers, and an adapter's profile as it stands on
 * the machine — the two things a phone could not ask about.
 *
 * ## What changed, stated as what the old comments said
 *
 * `StartAgentScreen`'s doc comment said, correctly at the time: "There is no
 * profile verb… There is no projects verb." Both are now on the wire —
 * `Request::Projects` and `Request::Profiles` in
 * `apex-agent-core/src/protocol.rs`, dispatched in `apex-agentd/src/main.rs`
 * beside `Worktrees`, and forwarded by `apex-remoted`, whose `control()`
 * refuses exactly two verbs (`attach`, `receive`) and neither of these.
 *
 * That claim is not made by reading. `android/core/src/test/resources/`
 * carries `requests.json` (what this app sends) and `projects-reply.json` and
 * `profiles-reply.json` (what it parses); the Kotlin tests in this module and
 * the Rust tests `android_requests_wire.rs` and `android_projects_wire.rs`
 * assert both ends against the same files, and `apex-agentd`'s own
 * `project_picker.rs` drives both verbs through a real daemon over a real
 * socket — including from a connection that has declared the origin
 * `claude-remote-control`, which is what a paired phone's requests arrive
 * under. A verb the daemon does not have fails to deserialize there; a verb it
 * refuses from a device fails the origin test.
 *
 * ## Why `projects` is a separate verb from `worktrees`, and matters
 *
 * `worktrees` answers with per-worktree git status for every remembered
 * project, which means running `git worktree list`, a rev walk and
 * `merge-tree --write-tree` in each of them. That cost is the documented
 * reason the Start screen had no picker even after the `worktrees` verb was
 * restored: seconds of git between a tap and a text field.
 *
 * `projects` reads one small JSON record per project and runs no subprocess.
 * So the Start screen asks THIS on open, and asks `worktrees` only for the one
 * project the user then chooses.
 */
@Serializable
data class ProjectRecord(
    /**
     * Absolute repository root.
     *
     * The field a client joins to [AgentSession.project], which is the project
     * root verbatim — not the name and not the slug. That join is how a
     * project row learns which sessions are working in it.
     */
    val root: String = "",
    /** Directory name, for display. */
    val name: String = "",
    /**
     * Stable identifier derived from the path.
     *
     * This — never the root — is what [Agentd.worktrees] takes. The daemon
     * resolves a slug by searching the remembered set, so the directories a
     * caller can make it run git in are exactly the ones the user already
     * chose to remember. A path there gets a `bad_request`.
     */
    val slug: String = "",
    /** Toolchains detected from marker files, sorted and deduplicated. */
    val languages: List<String> = emptyList(),
    /** Unix SECONDS this project was last used. Not milliseconds. */
    @SerialName("last_opened") val lastOpened: Long = 0,
    /**
     * The APEX capsule (§8) this project's work belongs in, or null.
     *
     * **This is the "workspace" P1-056 asks to browse.** There is no other
     * object in this runtime that is one: a capsule is the environment `apex
     * env` owns and a project may be bound to, and the binding is what makes
     * "which workspace does this work happen in" a question with an answer.
     * Null is the honest value for a project nobody has bound — which is every
     * project written before capsules existed — and it must render as "no
     * workspace", never as a neighbour's.
     */
    val capsule: String? = null,
) {
    /** Something to show when the daemon sent a record with no name. */
    val label: String get() = name.ifEmpty { root.substringAfterLast('/').ifEmpty { root } }

    /** Sessions from [sessions] working anywhere inside this project. */
    fun sessionsIn(sessions: List<AgentSession>): List<AgentSession> =
        sessions.filter { it.project == root }

    companion object {
        /**
         * Order for display: most recently opened first, then by name.
         *
         * The daemon already sorts this way and a client that re-sorted would
         * disagree with `apex project list`. Stated here anyway because the
         * order is a property the Start screen depends on — the first row is
         * the default selection — and a screen that depended on the daemon's
         * sort without saying so would silently pick a different project the
         * day the daemon's sort changed.
         */
        fun ordered(projects: List<ProjectRecord>): List<ProjectRecord> =
            projects.sortedWith(compareByDescending<ProjectRecord> { it.lastOpened }.thenBy { it.name })
    }
}

/**
 * One adapter, and its profile (§5) as it stands on the machine.
 *
 * One row per adapter `hello` lists, not one per profile: APEX describes a
 * profile for `claude` and for nothing else today, and a listing keyed on
 * profiles would answer "claude" to a client asking which agents it may start.
 *
 * ## The three fields that are worth the round trip
 *
 * * [programFound] — a phone that offered `codex` on a machine with no `codex`
 *   offered a button whose only outcome was the daemon's refusal. It is
 *   resolved on the DAEMON's `PATH`, which is the one that matters because the
 *   daemon is what spawns the agent, and which no client can derive.
 * * [commandRequired] — see below. It retires a hard-coded id.
 * * [installed] — an agent can be on `PATH` with no profile directory behind
 *   it. That is a startable session with none of the user's instructions,
 *   skills or commands in it, and it is worth saying so before the tap rather
 *   than after.
 *
 * ## What it deliberately does not carry
 *
 * No file names, no configured model, no plugin, marketplace, MCP server or
 * skill name, and no credential — not the value and not the name. The daemon
 * has all of that (`profile::doctor`) and does not send it; [secret] is a
 * COUNT of credential-class entries that exist. Tests on both sides assert the
 * absence against a fixture profile holding real-looking tokens, so a field
 * added later that carried one would fail rather than ship.
 */
@Serializable
data class AgentProfile(
    /** Adapter id — the value [Agentd.run]'s `agent` takes. */
    val agent: String = "",
    /** Human-facing name. */
    val display: String = "",
    /** The binary the adapter runs, or null for the one that runs anything. */
    val program: String? = null,
    /** Whether that binary resolves on the daemon's own `PATH`. */
    @SerialName("program_found") val programFound: Boolean = false,
    /**
     * Whether this adapter needs a program named by the caller.
     *
     * The flag this app used to derive as `agent == "generic"`, with a comment
     * saying the daemon published none. It does now, and it derives it from
     * the adapter table rather than from the id — so an adapter added later
     * that behaves the same way needs no app release. [Agentd.commandIsRequired]
     * still carries the id rule as the fallback for a daemon that predates
     * this field.
     */
    @SerialName("command_required") val commandRequired: Boolean = false,
    /** Whether APEX has a profile description for this adapter at all. */
    val described: Boolean = false,
    /** The profile directory, home-relative (`~/.claude`), when described. */
    val root: String? = null,
    /** Whether that directory exists on the machine. */
    val installed: Boolean = false,
    val reusable: Int = 0,
    val mixed: Int = 0,
    @SerialName("machine_local") val machineLocal: Int = 0,
    /** Credential-class entries present. A count; never a name, never a value. */
    val secret: Int = 0,
) {
    /**
     * Whether starting this adapter can possibly work, ignoring the command.
     *
     * `false` only when the daemon looked for the program and did not find it.
     * An adapter that needs a command has nothing to look for and is not
     * reported unstartable here — the Start screen's own `startIsPossible` is
     * where the command is required, so that a screen has one rule per
     * question instead of one rule that answers two and can only be wrong
     * about both.
     */
    val programIsPresent: Boolean get() = commandRequired || programFound

    /**
     * One line for the picker, or null when there is nothing worth saying.
     *
     * Null rather than "everything is fine": a row that always carries a
     * sentence trains the reader to stop reading it, and the one case that
     * matters — the agent is not on that machine — then reads like the rest.
     */
    val caution: String?
        get() = when {
            commandRequired -> "Runs whatever program you name."
            !programFound ->
                "${program ?: agent} is not on that machine's PATH, so starting it would fail."
            described && !installed ->
                "No $root on that machine: this would start $display with none of your " +
                    "instructions, skills or commands."
            else -> null
        }
}
