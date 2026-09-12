package com.apexos.remote.core.term

/**
 * The recorded TUI byte streams, and the one place their names live.
 *
 * Recorded on 2026-09-12 against a real PTY with `tools/record-tui-fixture.py`.
 * Nothing here is hand-written: each file is literally what the agent CLI wrote
 * to an 80x24 `xterm-256color` terminal in its first few seconds.
 *
 * Two recordings per agent, and the pair is the point:
 *
 * * `-queries` — against a terminal that answered **nothing**.
 * * `-session` — against one that answered, in the shapes xterm uses.
 *
 * **gemini is not installed on this machine**, so there is no gemini fixture.
 * P1-055 names four TUIs and this covers three; the fourth is untested and
 * saying so is cheaper than implying otherwise.
 */
object TuiFixtures {
    const val CLAUDE_QUERIES = "claude-queries"
    const val CLAUDE_SESSION = "claude-session"
    const val CODEX_QUERIES = "codex-queries"
    const val CODEX_SESSION = "codex-session"
    const val OPENCODE_QUERIES = "opencode-queries"
    const val OPENCODE_SESSION = "opencode-session"

    val ALL = listOf(
        CLAUDE_QUERIES, CLAUDE_SESSION,
        CODEX_QUERIES, CODEX_SESSION,
        OPENCODE_QUERIES, OPENCODE_SESSION,
    )

    /** The agents that have a fixture, with the pair of streams for each. */
    val AGENTS = mapOf(
        "claude" to (CLAUDE_QUERIES to CLAUDE_SESSION),
        "codex" to (CODEX_QUERIES to CODEX_SESSION),
        "opencode" to (OPENCODE_QUERIES to OPENCODE_SESSION),
    )

    fun read(name: String): ByteArray {
        val stream = TuiFixtures::class.java.getResourceAsStream("/tui/$name.bin")
            ?: error("the fixture /tui/$name.bin is missing; re-record it with tools/record-tui-fixture.py")
        return stream.use { it.readBytes() }
    }
}
