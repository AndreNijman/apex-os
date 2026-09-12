package com.apexos.remote.core.agent

/**
 * What is worth telling the user about, and once.
 *
 * ## There is no push transport, and this file does not pretend there is
 *
 * Measured, not assumed. There is no FCM, Firebase, UnifiedPush or ntfy
 * reference anywhere in this repository, `android/` included. There is no
 * subscribe, watch or follow verb on `apex-agentd`'s socket — the full
 * vocabulary is 28 verbs and `attach` is the only long-lived one, and it
 * streams raw PTY bytes rather than typed state. The relay is a stateless
 * byte-copier that has never been deployed (`relay/README.md:11`), both ends
 * dial out to it, and `apex-remote-core`'s `Frame` enum has no notification
 * tag. `apex-agentd` holds nothing open towards a phone and has nothing to
 * hold it open with.
 *
 * So an alert here is derived from **polling a connection the phone opened**,
 * and that is the honest description of it. The consequence for P1-058's
 * "notification content is encrypted/minimized so push infrastructure does not
 * receive sensitive prompt/code content" is worth stating plainly rather than
 * dressing up: **no push infrastructure receives anything, because there is
 * none.** What reaches the phone is what already reaches it — the `list` reply
 * inside the Noise session. [Alert] still carries the minimum, so that the day
 * a wake channel exists there is no payload here that must not cross it.
 *
 * ## What must never be in an alert
 *
 * `SessionInfo.detail` is built by `hook::detail_for` and copies **verbatim**,
 * clipped to 120 characters: Bash command lines, file paths, grep patterns,
 * fetched URLs, task descriptions, and Claude's own notification text. It is
 * the single most sensitive field on the record and it is never carried here.
 * Nor are `cwd`, `project`, `worktree`, `args` or `telemetry.branch`, which
 * name what the user is working on.
 *
 * An [Alert] is `(machine, session id, kind, agent)`. The words the user reads
 * come from [AgentStates.label] and this file, both of which are fixed strings
 * in this build.
 */
data class Alert(
    /** Which machine. A paired device id, opaque and already known to the phone. */
    val machine: String,
    /** The session, within that machine. See [Key] for why it is not enough alone. */
    val session: Int,
    val kind: Kind,
    /** The adapter: `claude`, `codex`. Not the program, not the prompt. */
    val agent: String,
    /** Unix **milliseconds** the alert was raised on the phone. */
    val atMs: Long,
) {
    /**
     * What happened, in the vocabulary P1-058 asks for.
     *
     * Three of the six criteria map onto a session state the daemon publishes;
     * the other three do not, and are named here as what they actually are.
     */
    enum class Kind(val title: String, val detail: String) {
        /**
         * The agent is waiting for the user.
         *
         * **This is also what "working complete" arrives as**, and that is a
         * property of the runtime rather than a simplification here.
         * `AgentState::Complete` comes only from the agent process exiting 0
         * (`session.rs:493`), and the `claude` adapter runs claude
         * interactively — the prompt is a trailing argument, not `-p` — so a
         * managed session reaches `complete` only when a human quits claude.
         * The end of a turn publishes `waiting_for_user`, via the `stop` hook.
         * A separate "finished" alert would therefore either never fire or
         * fire for every turn under a name that means something else.
         */
        WAITING("Waiting for you", "The agent finished its turn and wants input."),

        /** The agent is asking for permission for something it wants to do. */
        PERMISSION("Needs permission", "The agent is asking before it acts."),

        /** The session's process exited non-zero. */
        FAILED("Failed", "The session exited with an error."),

        /** The session's process exited 0, or was ended. */
        FINISHED("Finished", "The session ended."),

        /**
         * A test run APEX observed in one of this machine's worktrees failed.
         *
         * Not a session state: it reaches the daemon as a `TestNote` on a hook
         * event and is stored per worktree path, surfacing on
         * `WorktreeStatus.tests` rather than on any session. So this kind is
         * raised from a `worktrees` poll, not from `list`, and its `session`
         * is the first session the daemon attributes to that worktree — or
         * [NO_SESSION] when it attributes none.
         */
        TEST_FAILED("Tests failed", "The last test run in a worktree failed."),

        /**
         * A privileged operation is waiting for a decision.
         *
         * The phone cannot make that decision: see
         * [PrivilegeRequest.DECIDE_IS_LOCAL_ONLY]. The alert exists so the user
         * knows to go to the machine, which is the only thing it can usefully
         * cause.
         */
        APPROVAL("Approval needed", "A privileged operation is waiting at the machine."),
    }

    /**
     * What two alerts must share to be the same alert.
     *
     * **The duplicate this deduplicates is internal to APEX, not "Claude
     * versus APEX".** Four independent paths publish `waiting_for_user` for
     * one moment: the `notification` hook, the `stop` hook, the PTY scanner
     * seeing a BEL or an OSC 9/777, and the ten-second idle rule
     * (`session.rs:484`). `Session::set_state` (`registry.rs:163`) has **no
     * same-state early return** — it assigns unconditionally and bumps
     * `last_activity` — so a poller keying on "saw waiting_for_user" fires up
     * to four times for one turn, seconds apart. And `SessionInfo` carries no
     * field naming the hook event that caused the transition, so the four are
     * not distinguishable at the client even in principle.
     *
     * Dedup is therefore on the **transition edge**: an alert is raised when a
     * session ENTERS a state, and not again while it stays there. That is the
     * only rule available, and it happens to be the right one.
     *
     * The machine is part of the key because `SessionInfo.id` is a per-daemon
     * counter: two paired machines both have a session 1.
     */
    data class Key(val machine: String, val session: Int, val kind: Kind)

    val key: Key get() = Key(machine, session, kind)

    companion object {
        /** A test-failure alert for a worktree the daemon attributes no session to. */
        const val NO_SESSION: Int = -1
    }
}

/**
 * Turns successive polls into alerts, raising each transition once.
 *
 * Deliberately a plain class in `:core` with no Android type in it: everything
 * that can be a rule lives where a JVM test can reach it, because nothing in
 * this project can test a Compose callback or a `NotificationManager`.
 *
 * **Not thread-safe, and it does not need to be.** It is driven from one
 * polling loop. A caller that drives it from two would get interleaved state,
 * which is why [observe] takes the whole world each time rather than one
 * session at a time.
 */
class AlertWatcher {
    /** The state each session was last seen in, per machine. */
    private val lastState = mutableMapOf<Pair<String, Int>, String>()

    /** Worktree path -> the test state last seen there, per machine. */
    private val lastTests = mutableMapOf<Pair<String, String>, String>()

    /** Privilege request ids already announced, per machine. */
    private val announced = mutableSetOf<Pair<String, Int>>()

    /** Whether anything has been observed yet for a machine. */
    private val seen = mutableSetOf<String>()

    /**
     * Fold one poll of one machine into alerts.
     *
     * **The first poll of a machine raises nothing.** A phone opening the app
     * to six sessions that have been waiting since yesterday must not fire six
     * notifications for things that did not just happen; a notification means
     * "this changed", and on the first look nothing has. Recorded as a rule
     * rather than as an accident of an empty map, because an empty map would
     * also be the state after a process restart — which is exactly when the
     * six-notification burst would happen.
     *
     * @param nowMs the clock, passed in because a test cannot pin one it does
     *   not hold.
     */
    fun observe(
        machine: String,
        sessions: List<AgentSession>,
        worktrees: List<WorktreeStatus> = emptyList(),
        requests: List<PrivilegeRequest> = emptyList(),
        nowMs: Long = 0,
    ): List<Alert> {
        val first = seen.add(machine)
        val out = mutableListOf<Alert>()

        for (s in sessions) {
            val at = machine to s.id
            val was = lastState.put(at, s.state)
            if (first || was == s.state) continue
            val kind = kindOf(s) ?: continue
            out += Alert(machine, s.id, kind, s.agent, nowMs)
        }

        // A session the daemon has forgotten must not keep its old state in
        // this map: `Request::Remove` and `Prune` make the id reusable
        // (`registry.rs:441`), and a new session landing on a recycled id
        // would be compared against a stranger's state.
        val live = sessions.map { machine to it.id }.toSet()
        lastState.keys.retainAll { it.first != machine || it in live }

        for (w in worktrees) {
            val at = machine to w.path
            val was = lastTests.put(at, w.tests.state)
            if (first || was == w.tests.state || !w.tests.isFailed) continue
            out += Alert(
                machine,
                w.sessions.firstOrNull() ?: Alert.NO_SESSION,
                Alert.Kind.TEST_FAILED,
                sessions.firstOrNull { it.id in w.sessions }?.agent ?: "",
                nowMs,
            )
        }

        for (r in requests) {
            if (!r.isPending) {
                // Decided elsewhere. Forgotten so that a request re-opened by
                // a daemon restart announces itself again.
                announced.remove(machine to r.id)
                continue
            }
            if (!announced.add(machine to r.id)) continue
            if (first) continue
            out += Alert(machine, r.session ?: Alert.NO_SESSION, Alert.Kind.APPROVAL, r.agent ?: "", nowMs)
        }

        return out
    }

    /** Forget a machine, so the next poll of it is a first poll again. */
    fun forget(machine: String) {
        seen.remove(machine)
        lastState.keys.retainAll { it.first != machine }
        lastTests.keys.retainAll { it.first != machine }
        announced.retainAll { it.first != machine }
    }

    private fun kindOf(s: AgentSession): Alert.Kind? = when (s.state) {
        AgentStates.WAITING_FOR_USER -> Alert.Kind.WAITING
        AgentStates.PERMISSION_REQUEST -> Alert.Kind.PERMISSION
        AgentStates.FAILED -> Alert.Kind.FAILED
        AgentStates.COMPLETE, AgentStates.EXITED -> Alert.Kind.FINISHED
        // `starting` and `working` are not news.
        else -> null
    }
}
