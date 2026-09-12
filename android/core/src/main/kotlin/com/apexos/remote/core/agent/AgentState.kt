package com.apexos.remote.core.agent

/**
 * What an agent session's state looks like — the phone's half of a table the
 * desktop already owns.
 *
 * ## This is a port, and the port is the risk
 *
 * `apex-shell/src/services/agentstate.js` is the single source of this mapping
 * on the desktop, and its own header records why it exists: the same
 * three-branch ternary had been written twice, in `SessionRow.qml` and
 * `RemoteSessionRow.qml`, and the two drifted. Adding a third copy in Kotlin
 * is exactly that mistake again unless something checks the two agree — so
 * `AgentStateAgreementTest` parses the real JavaScript out of
 * `core/src/test/resources/desktop/agentstate.js` (a verbatim copy, with its
 * provenance recorded beside it) and asserts this file matches it in both
 * directions.
 *
 * ## The tokens are names, not colours
 *
 * Nothing here is a hex. `info`, `warning`, `attention`, `danger`, `success`
 * and `subtext` are theme slots, resolved by whichever renderer is drawing —
 * which is what lets the desktop's light scheme and the phone's dark one carry
 * the same meaning without this table knowing about either.
 *
 * ## Two rules worth stating, because both look like bugs
 *
 * * `exited` is **not** failure-toned. A session that ended is not a session
 *   that failed, and `apex-agent-core/src/session.rs` is explicit that signal
 *   death is not failure.
 * * `starting` shares `working`'s tone deliberately. They differ by glyph, not
 *   by colour: a session that has just started is working, and flashing a
 *   different colour for the first half-second would be noise.
 */
enum class Tone(val token: String, val weight: Weight) {
    WORKING("info", Weight.TINT),
    WAITING("warning", Weight.OUTLINE),
    BLOCKED("attention", Weight.SOLID),
    FAILED("danger", Weight.SOLID),
    DONE("success", Weight.PLAIN),
    IDLE("subtext", Weight.PLAIN),
    ;

    /** The lower-case name the desktop's table uses. */
    val wire: String get() = name.lowercase()
}

/**
 * How a state badge is filled.
 *
 * The desktop's `StateBadge.qml` derives all three of tone, weight and fill
 * from `sessionState` alone — it takes no agent parameter at all, which is
 * what makes every session kind render identically. The same is true here.
 */
enum class Weight {
    /** Tone as the fill, glyph in a contrast-picked ink. */
    SOLID,

    /** Tone at [TINT_ALPHA], glyph in the tone. */
    TINT,

    /** A one-pixel ring in the tone, no fill. */
    OUTLINE,

    /** No chip at all. */
    PLAIN,
    ;

    val wire: String get() = name.lowercase()

    companion object {
        /** `StateBadge.qml`: `Qt.rgba(c.r, c.g, c.b, 0.18)`. */
        const val TINT_ALPHA: Float = 0.18f
    }
}

/**
 * The seven states `apex-agent-core`'s `AgentState` can publish, plus the rule
 * for the eighth.
 *
 * The wire strings are `#[serde(rename_all = "snake_case")]` on the Rust enum,
 * which is why `waiting_for_user` and `permission_request` have underscores
 * and nothing here is spelled by hand twice.
 */
object AgentStates {
    const val STARTING = "starting"
    const val WORKING = "working"
    const val WAITING_FOR_USER = "waiting_for_user"
    const val PERMISSION_REQUEST = "permission_request"
    const val COMPLETE = "complete"
    const val FAILED = "failed"
    const val EXITED = "exited"

    /** Every state the runtime can publish, in the Rust enum's order. */
    val ALL = listOf(STARTING, WORKING, WAITING_FOR_USER, PERMISSION_REQUEST, COMPLETE, FAILED, EXITED)

    private val TONES = mapOf(
        STARTING to Tone.WORKING,
        WORKING to Tone.WORKING,
        WAITING_FOR_USER to Tone.WAITING,
        PERMISSION_REQUEST to Tone.BLOCKED,
        COMPLETE to Tone.DONE,
        FAILED to Tone.FAILED,
        EXITED to Tone.IDLE,
    )

    /**
     * The tone for a state, or [Tone.IDLE] for one this build has not been
     * taught.
     *
     * Idle and **never** failed. An unknown state means a runtime newer than
     * this app, and drawing it as a fault would report a failure the runtime
     * never claimed.
     */
    fun tone(state: String?): Tone = TONES[state] ?: Tone.IDLE

    fun token(state: String?): String = tone(state).token

    fun weight(state: String?): Weight = tone(state).weight

    /**
     * Whether this state is waiting on the person holding the phone.
     *
     * The one predicate worth having separately: it is what a notification, a
     * badge count and the order of a list should all key on, and deriving it
     * from the colour would tie three behaviours to a palette.
     */
    fun needsYou(state: String?): Boolean = state == WAITING_FOR_USER || state == PERMISSION_REQUEST

    /** Whether the session has finished, by any route. `AgentState::is_terminal`. */
    fun isTerminal(state: String?): Boolean = state == COMPLETE || state == FAILED || state == EXITED

    /** The five tones a session can be in that are not "idle". */
    val DISTINCT_TONES = listOf(Tone.WORKING, Tone.WAITING, Tone.BLOCKED, Tone.FAILED, Tone.DONE)

    /** What a person is shown instead of the wire string. */
    private val LABELS = mapOf(
        STARTING to "starting",
        WORKING to "working",
        WAITING_FOR_USER to "waiting for you",
        PERMISSION_REQUEST to "needs permission",
        COMPLETE to "complete",
        FAILED to "failed",
        EXITED to "exited",
    )

    /** A state a person can read. An unknown one is shown as itself, never hidden. */
    fun label(state: String?): String = LABELS[state] ?: (state ?: "unknown")
}

/**
 * Display names for the agent adapters.
 *
 * From `agentstate.js`'s `AGENT_NAMES`. An unknown id is title-cased rather
 * than dropped: a runtime that grows a new adapter should show its name, not
 * a blank.
 */
object AgentNames {
    private val NAMES = mapOf(
        "claude" to "Claude",
        "opencode" to "OpenCode",
        "codex" to "Codex",
        "gemini" to "Gemini",
        "kimi" to "Kimi",
        "generic" to "Agent",
    )

    fun of(agent: String?): String {
        val id = agent?.lowercase()?.trim()
        if (id.isNullOrEmpty()) return NAMES.getValue("generic")
        NAMES[id]?.let { return it }
        return id.replaceFirstChar { it.uppercase() }
    }
}
