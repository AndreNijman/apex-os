package com.apexos.remote.core.agent

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

/**
 * P1-054: "agent state and colours match desktop semantics."
 *
 * ## Why this reads JavaScript
 *
 * Because "match" is a claim about two files, and a claim about two files that
 * nothing checks is the defect this program keeps finding. `agentstate.js`'s
 * own header records the last time it happened: the same three-branch ternary
 * lived in `SessionRow.qml` and in `RemoteSessionRow.qml`, and they drifted.
 * A Kotlin copy is the third instance of that table, so it comes with the
 * thing the other two eventually needed.
 *
 * So this test parses the **real desktop source** — a verbatim copy, with its
 * repo, commit and sha256 recorded in `desktop/PROVENANCE.md` beside it — and
 * asserts the Kotlin agrees with it in both directions: every state the
 * JavaScript knows maps to the same token and weight here, and this file
 * invents no state the JavaScript has never heard of.
 *
 * ## What it cannot prove, said plainly
 *
 * The two live in different repositories and neither CI checks out the other,
 * so this proves agreement with a **snapshot**. `tools/check-agent-state.sh`
 * closes the rest of the gap when an apex-shell checkout is available, and
 * prints NOT CHECKED when it is not — rather than reporting success because it
 * could not look.
 */
class AgentStateAgreementTest {
    private val source: String by lazy {
        AgentStateAgreementTest::class.java.getResourceAsStream("/desktop/agentstate.js")
            ?.use { it.readBytes().toString(Charsets.UTF_8) }
            ?: error("the vendored agentstate.js is missing from core/src/test/resources/desktop/")
    }

    /** `var NAME = { key: "value", … }` — the shape both tables are written in. */
    private fun objectLiteral(name: String): Map<String, String> {
        val body = Regex("""var\s+$name\s*=\s*\{(.*?)\}""", RegexOption.DOT_MATCHES_ALL)
            .find(source)?.groupValues?.get(1)
            ?: error("$name is not in the vendored agentstate.js; the file has been restructured")
        return Regex("""(\w+)\s*:\s*"([^"]*)"""").findAll(body)
            .associate { it.groupValues[1] to it.groupValues[2] }
    }

    /** `var TONES = { working: { token: "info", weight: "tint" }, … }` */
    private fun nestedLiteral(name: String): Map<String, Map<String, String>> {
        val body = Regex("""var\s+$name\s*=\s*\{(.*?)\n\}""", RegexOption.DOT_MATCHES_ALL)
            .find(source)?.groupValues?.get(1)
            ?: error("$name is not in the vendored agentstate.js")
        return Regex("""(\w+)\s*:\s*\{([^}]*)\}""").findAll(body).associate { entry ->
            entry.groupValues[1] to Regex("""(\w+)\s*:\s*"([^"]*)"""")
                .findAll(entry.groupValues[2])
                .associate { it.groupValues[1] to it.groupValues[2] }
        }
    }

    @Test
    fun `the parser found real tables and not empty ones`() {
        // The anti-vacuity guard, and it goes first. Every assertion below
        // iterates the parsed tables; a regex that matched nothing would make
        // all of them pass against any Kotlin at all.
        val states = objectLiteral("STATE_TONES")
        val tones = nestedLiteral("TONES")
        assertEquals(7, states.size, "STATE_TONES parsed as $states")
        assertEquals(6, tones.size, "TONES parsed as ${tones.keys}")
        assertTrue(source.length > 2000, "the vendored file is suspiciously small")
    }

    @Test
    fun `every state the desktop knows maps to the same tone here`() {
        val states = objectLiteral("STATE_TONES")
        for ((state, toneName) in states) {
            assertEquals(
                toneName,
                AgentStates.tone(state).wire,
                "the desktop draws `$state` as `$toneName`; this build draws it as " +
                    "`${AgentStates.tone(state).wire}`",
            )
        }
    }

    @Test
    fun `every tone resolves to the same token and the same weight`() {
        val tones = nestedLiteral("TONES")
        for (tone in Tone.entries) {
            val desktop = tones[tone.wire] ?: error("the desktop has no tone `${tone.wire}`")
            assertEquals(desktop["token"], tone.token, "token for tone `${tone.wire}`")
            assertEquals(desktop["weight"], tone.weight.wire, "weight for tone `${tone.wire}`")
        }
        // And the other direction: a tone the desktop has and this build does
        // not would render as nothing at all.
        for (name in tones.keys) {
            assertTrue(
                Tone.entries.any { it.wire == name },
                "the desktop has a tone `$name` that this build has never heard of",
            )
        }
    }

    @Test
    fun `this build invents no state the desktop has never heard of`() {
        val states = objectLiteral("STATE_TONES").keys
        assertEquals(
            states.sorted(),
            AgentStates.ALL.sorted(),
            "the two lists of runtime states differ",
        )
    }

    @Test
    fun `an unknown state is idle on both sides and never a failure`() {
        // The rule the JavaScript states in a comment: a state this file has
        // not been taught is a runtime newer than the client, and drawing it
        // as "failed" would report a fault the runtime never claimed.
        assertTrue(source.contains("|| \"idle\""), "the desktop's fallback is no longer `idle`")
        assertEquals(Tone.IDLE, AgentStates.tone("a_state_from_2027"))
        assertEquals(Tone.IDLE, AgentStates.tone(null))
        assertEquals(Tone.IDLE, AgentStates.tone(""))
        assertFalse(AgentStates.needsYou("a_state_from_2027"))
    }

    @Test
    fun `the five distinct tones really are distinct, as the desktop asserts`() {
        val listed = Regex("""var\s+DISTINCT_TONES\s*=\s*\[(.*?)\]""", RegexOption.DOT_MATCHES_ALL)
            .find(source)?.groupValues?.get(1)
            ?.let { Regex("\"([^\"]+)\"").findAll(it).map { m -> m.groupValues[1] }.toList() }
            ?: error("DISTINCT_TONES is not in the vendored file")
        assertEquals(listed, AgentStates.DISTINCT_TONES.map { it.wire })
        assertEquals(
            listed.size,
            AgentStates.DISTINCT_TONES.map { it.token }.toSet().size,
            "two of the five distinct tones share a token, so two states look the same",
        )
    }

    @Test
    fun `exited is idle and not failed, and starting shares working's tone`() {
        // Two rules that look like bugs and are not, so they are named:
        // signal death is not failure, and a session that has just started is
        // working — the two differ by glyph, not by colour.
        assertEquals(Tone.IDLE, AgentStates.tone(AgentStates.EXITED))
        assertEquals(AgentStates.tone(AgentStates.WORKING), AgentStates.tone(AgentStates.STARTING))
    }

    @Test
    fun `the agent display names match the desktop's`() {
        val names = objectLiteral("AGENT_NAMES")
        assertTrue(names.size >= 5, "AGENT_NAMES parsed as $names")
        for ((id, display) in names) {
            assertEquals(display, AgentNames.of(id), "display name for `$id`")
        }
        // An adapter neither side knows is title-cased rather than dropped.
        assertEquals("Mistral", AgentNames.of("mistral"))
        assertEquals("Agent", AgentNames.of(null))
        assertEquals("Agent", AgentNames.of(""))
    }

    @Test
    fun `needsYou means the same on both sides`() {
        assertTrue(source.contains("waiting_for_user") && source.contains("permission_request"))
        assertTrue(AgentStates.needsYou(AgentStates.WAITING_FOR_USER))
        assertTrue(AgentStates.needsYou(AgentStates.PERMISSION_REQUEST))
        for (state in AgentStates.ALL) {
            if (state == AgentStates.WAITING_FOR_USER || state == AgentStates.PERMISSION_REQUEST) continue
            assertFalse(AgentStates.needsYou(state), "`$state` must not claim to need a person")
        }
    }

    @Test
    fun `the badge fill rules match StateBadge's`() {
        // Weights are what StateBadge turns into geometry: solid fills, tint
        // fills at 0.18, outline draws a ring, plain draws no chip at all.
        assertEquals(Weight.SOLID, AgentStates.weight(AgentStates.PERMISSION_REQUEST))
        assertEquals(Weight.SOLID, AgentStates.weight(AgentStates.FAILED))
        assertEquals(Weight.TINT, AgentStates.weight(AgentStates.WORKING))
        assertEquals(Weight.OUTLINE, AgentStates.weight(AgentStates.WAITING_FOR_USER))
        assertEquals(Weight.PLAIN, AgentStates.weight(AgentStates.COMPLETE))
        assertEquals(Weight.PLAIN, AgentStates.weight(AgentStates.EXITED))
        assertEquals(0.18f, Weight.TINT_ALPHA)
    }
}
