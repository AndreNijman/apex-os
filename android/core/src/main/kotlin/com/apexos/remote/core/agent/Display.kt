package com.apexos.remote.core.agent

/**
 * The parts of an Agent Center that are decisions rather than layout.
 *
 * Which session is at the top, how long "two hours and eleven minutes" is
 * written, what a session with no telemetry shows, and which processes hang
 * off which — every one of those is a rule the desktop already has, and every
 * one of them is testable without a phone.
 *
 * Where the desktop has a rule, it is ported rather than reinvented, and the
 * source is named at each one. P1-054's last criterion is about matching the
 * desktop, and "the phone sorts its own way" is the same defect as "the phone
 * picks its own colours".
 */

/**
 * Whether a session is running, by the desktop's own test.
 *
 * `AgentService._isLive`: **`exit_code === null && exit_signal === null`**, and
 * deliberately not `!isTerminal(state)`. The two disagree, and the
 * disagreement is not hypothetical — `state` is whatever was last *published*,
 * including by the agent itself through the open `Event` verb, while the exit
 * fields are what the daemon observed when the process ended. A session that
 * published `complete` and is still running is live; one that was killed
 * before it published anything is not.
 *
 * Sorting on the published state would let an agent put itself at the top of
 * somebody's list by reporting `working` forever.
 */
val AgentSession.live: Boolean get() = exitCode == null && exitSignal == null

/**
 * The Agent Center's order, from `AgentCenter.qml` and `remoteagents.js`.
 *
 * Two rules and they are in this order:
 *
 * 1. **The ones that need you, first, as their own group.** The desktop
 *    filters them into a separate section above everything else rather than
 *    sorting them to the top, and the difference matters on a phone: a group
 *    can carry a heading, and a heading is what makes "two agents are waiting
 *    for you" legible without counting badges.
 * 2. **Then live before finished, then most recently active.** The desktop's
 *    comment says why: "a finished session sinking below a running one is what
 *    makes the list readable at a glance."
 *
 * Within the needs-you group the desktop does not sort at all, and neither
 * does this: the daemon lists sessions oldest first, so the one that has been
 * waiting longest is at the top of the group, which is the right answer and
 * costs nothing.
 */
object Order {
    /** The two groups, in the order they are drawn. */
    fun groups(sessions: List<AgentSession>): Pair<List<AgentSession>, List<AgentSession>> {
        val needsYou = sessions.filter { it.needsYou }
        val rest = sessions.filterNot { it.needsYou }.sortedWith(REST)
        return needsYou to rest
    }

    /** Both groups as one list, for anything that cannot draw a heading. */
    fun flat(sessions: List<AgentSession>): List<AgentSession> {
        val (needsYou, rest) = groups(sessions)
        return needsYou + rest
    }

    /**
     * Live first, then `last_activity` descending.
     *
     * `compareByDescending` on a `Boolean` puts `true` first, which is what
     * "live first" means, and the second key is descending so the newest is at
     * the top. Written as a comparator rather than inline so both callers use
     * the same one — which is the mistake `agentstate.js` exists because of,
     * in a smaller costume.
     */
    val REST: Comparator<AgentSession> =
        compareByDescending<AgentSession> { it.live }.thenByDescending { it.lastActivity }

    /** How many sessions are running. `AgentService.liveCount`. */
    fun liveCount(sessions: List<AgentSession>): Int = sessions.count { it.live }

    /** How many want a decision. `AgentService.attentionCount`, without the privilege requests. */
    fun attentionCount(sessions: List<AgentSession>): Int = sessions.count { it.needsYou }
}

/**
 * Elapsed time, written the way the desktop writes it.
 *
 * `AgentService.elapsed`: coarse and short — seconds under a minute, whole
 * minutes under an hour, then hours and minutes. A supervisor's list is
 * scanned, not read, and `01:47:23` is three numbers to parse where `1h 47m`
 * is one fact.
 *
 * The rule that looks like a bug and is not: **a finished session's clock
 * stops at its last activity**, not at now. A session that failed an hour ago
 * did not take an hour; it took however long it ran. The desktop computes
 * `end = live ? now : (last_activity || started)` and this does the same.
 */
object Elapsed {
    /** The desktop's string for a session, given the current time in **seconds**. */
    fun of(session: AgentSession, nowSeconds: Long): String {
        if (session.started <= 0) return ""
        val end = if (session.live) {
            nowSeconds
        } else {
            if (session.lastActivity > 0) session.lastActivity else session.started
        }
        return format(maxOf(0, end - session.started))
    }

    /** Seconds as the desktop renders them. */
    fun format(seconds: Long): String {
        if (seconds < 60) return "${seconds}s"
        if (seconds < 3600) return "${seconds / 60}m"
        val h = seconds / 3600
        val m = (seconds % 3600) / 60
        return if (m > 0) "${h}h ${m}m" else "${h}h"
    }

    /** The same, from milliseconds, for a caller holding [AgentSession.elapsedMs]. */
    fun formatMs(ms: Long): String = format(ms / 1000)
}

/**
 * The glyph for a state, from `AgentService.stateIcons`.
 *
 * Nerd Font private-use code points, which is what the shell draws them with.
 * A phone has no Nerd Font unless one is bundled, so these are carried for
 * agreement and a renderer is free to ignore them — [StateGlyphs.ascii] is the
 * fallback, and it is a fallback rather than the only option so that an APEX
 * phone with the font installed looks like the laptop.
 */
object StateGlyphs {
    private val NERD = mapOf(
        AgentStates.STARTING to "󰉭",
        AgentStates.WORKING to "󰜎",
        AgentStates.WAITING_FOR_USER to "󰅺",
        AgentStates.PERMISSION_REQUEST to "󰜾",
        AgentStates.COMPLETE to "󰄬",
        AgentStates.FAILED to "󰅚",
        AgentStates.EXITED to "󰩈",
    )

    /** The unknown-state glyph the desktop falls back to. */
    const val UNKNOWN_NERD: String = "󰘦"

    fun nerd(state: String?): String = NERD[state] ?: UNKNOWN_NERD

    /**
     * A glyph from the fonts every phone has.
     *
     * Chosen so that the five distinct tones differ in **shape** as well as in
     * colour, which is the whole reason `agentstate.js` carries a weight: the
     * pair a red-green colourblind reader cannot separate by hue is `failed`
     * and `done`, and a tick against a cross is unambiguous to everybody.
     */
    private val ASCII = mapOf(
        AgentStates.STARTING to "○", // ○ an empty circle: about to start
        AgentStates.WORKING to "●", // ● filled: producing output
        AgentStates.WAITING_FOR_USER to "○", // ○ same shape, outline weight
        AgentStates.PERMISSION_REQUEST to "!",
        AgentStates.COMPLETE to "✓", // ✓
        AgentStates.FAILED to "✕", // ✕
        AgentStates.EXITED to "–", // – ended, and not a fault
    )

    fun ascii(state: String?): String = ASCII[state] ?: "?"
}

/**
 * A telemetry figure, or the absence of one.
 *
 * `Telemetry`'s own documentation says it: every field is optional and `null`
 * means **we have never heard**, which is not zero. "A context gauge drawn at
 * 0% for a session that has never reported is a gauge that is lying."
 *
 * This exists so that rule is a function with a test rather than a `?:0`
 * somebody writes at a call site at four in the afternoon.
 */
object Gauge {
    /**
     * A percentage as a fraction 0..1, or `null` when nothing has been heard.
     *
     * Out-of-range values are clamped rather than refused: a runtime that
     * reports 104% has a bug, and a phone that drew nothing would hide a
     * session that is in trouble.
     */
    fun fraction(pct: Double?): Float? = pct?.let { (it / 100.0).coerceIn(0.0, 1.0).toFloat() }

    /** `"62%"`, or `null`. Never `"0%"` for a session that has not reported. */
    fun label(pct: Double?): String? = pct?.let { "${Math.round(it)}%" }

    /**
     * Whether the two rate-limit figures should be drawn on a session row.
     *
     * They should not, and this returns false for a reason the daemon states:
     * they are **account-wide, not per session**. Six sessions on one login
     * report the same number, and a list that drew it per row would repeat one
     * fact six times.
     */
    const val RATE_LIMITS_ARE_PER_ROW: Boolean = false
}
