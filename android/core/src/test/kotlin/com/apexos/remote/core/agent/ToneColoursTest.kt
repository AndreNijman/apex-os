package com.apexos.remote.core.agent

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertNotNull
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

/**
 * The other half of "colours match desktop semantics".
 *
 * `AgentStateAgreementTest` proves state -> token. This proves token ->
 * colour, and it does it the same way and for the same reason: by parsing the
 * **real desktop source**. `Colors.qml` is vendored verbatim beside
 * `agentstate.js`, from the same apex-shell commit, and its provenance is in
 * `desktop/PROVENANCE.md`.
 *
 * Without this, `Tones.kt` would be ten hex literals that looked like the
 * desktop's on the day they were typed.
 */
class ToneColoursTest {
    private val source: String by lazy {
        ToneColoursTest::class.java.getResourceAsStream("/desktop/Colors.qml")
            ?.use { it.readBytes().toString(Charsets.UTF_8) }
            ?: error("the vendored Colors.qml is missing from core/src/test/resources/desktop/")
    }

    /**
     * `property color NAME: darkSurface ? "#aaa" : "#bbb"` -> the two hexes.
     *
     * The ternary is parsed rather than the whole line matched loosely,
     * because a token that stopped being a pair — somebody deciding `success`
     * can be one colour again — must fail here loudly rather than silently
     * match the first hex on the line.
     */
    private fun pair(name: String): Pair<Int, Int> {
        val m = Regex("""property\s+color\s+$name\s*:\s*darkSurface\s*\?\s*"(#[0-9a-fA-F]{6})"\s*:\s*"(#[0-9a-fA-F]{6})"""")
            .find(source)
            ?: error("$name is not a darkSurface ternary in the vendored Colors.qml; the theme has been restructured")
        return hex(m.groupValues[1]) to hex(m.groupValues[2])
    }

    private fun plain(name: String): Int {
        val m = Regex("""property\s+color\s+$name\s*:\s*"(#[0-9a-fA-F]{6})"""").find(source)
            ?: error("$name is not a fixed hex in the vendored Colors.qml")
        return hex(m.groupValues[1])
    }

    private fun hex(text: String): Int = text.removePrefix("#").toInt(16)

    @Test
    fun `every status token carries the desktop's two hexes, exactly`() {
        // The five with a colour. `subtext` is deliberately absent from both
        // this list and `Tones`: it is the palette's own muted foreground and
        // a hex for it here would be a third opinion about the palette.
        for (token in listOf("danger", "warning", "success", "info", "attention")) {
            val (dark, light) = pair(token)
            assertEquals(
                dark,
                Tones.of(token, darkSurface = true),
                "$token on a dark surface disagrees with apex-shell: " +
                    "desktop #%06x, here #%06x".format(dark, Tones.of(token, true) ?: 0),
            )
            assertEquals(
                light,
                Tones.of(token, darkSurface = false),
                "$token on a light surface disagrees with apex-shell: " +
                    "desktop #%06x, here #%06x".format(light, Tones.of(token, false) ?: 0),
            )
        }
    }

    @Test
    fun `the table holds nothing the desktop does not define`() {
        // The other direction, which is what catches a token invented here.
        for (token in Tones.DARK.keys) {
            assertTrue(
                Regex("""property\s+color\s+$token\s*:""").containsMatchIn(source),
                "`$token` is in the Kotlin table and is not a colour apex-shell defines",
            )
        }
    }

    @Test
    fun `the two fixed foregrounds are the desktop's`() {
        assertEquals(plain("fixedLight"), Tones.FIXED_LIGHT)
        assertEquals(plain("fixedDark"), Tones.FIXED_DARK)
    }

    @Test
    fun `darkSurface is the desktop's threshold and reads the surface, not a flag`() {
        val threshold = Regex("""luminance\(root\.background\)\s*<\s*([0-9.]+)""").find(source)
            ?.groupValues?.get(1)?.toDouble()
            ?: error("`darkSurface` is no longer a luminance comparison in the vendored Colors.qml")
        assertEquals(threshold, Tones.DARK_SURFACE_BELOW)

        // The APEX ground, and Material You's palest plausible "dark" surface.
        assertTrue(Tones.darkSurface(0x1A282A), "the greeter's ground is a dark surface")
        assertFalse(Tones.darkSurface(0xF4F7F7), "the paper scheme is not a dark surface")
        // The case that motivates reading a colour rather than a theme flag:
        // a dynamic scheme's surface can be light while the phone is nominally
        // in dark mode.
        assertFalse(Tones.darkSurface(0xFAF9FB), "matugen's light surface is not a dark surface")
    }

    @Test
    fun `onStatus picks the better-contrasting ink, and beats a lightness threshold`() {
        // The exact case `Colors.qml` names: a 0.35 threshold sends white onto
        // `#f87171` at 2.8:1, where the dark ink reads 5.9:1.
        val danger = Tones.of("danger", darkSurface = true)!!
        assertEquals(Tones.FIXED_DARK, Tones.onStatus(danger))
        assertTrue(
            Tones.contrastRatio(danger, Tones.onStatus(danger)) >
                Tones.contrastRatio(danger, Tones.FIXED_LIGHT),
            "onStatus picked the worse of the two inks",
        )
        // And the other way, on the light scheme's deep red.
        val deepRed = Tones.of("danger", darkSurface = false)!!
        assertEquals(Tones.FIXED_LIGHT, Tones.onStatus(deepRed))
    }

    @Test
    fun `a solid badge's glyph clears 4_5 to 1 against its own fill`() {
        // The two SOLID weights are `blocked` and `failed` — the states that
        // mean somebody has to do something, so they are the two whose glyph
        // must not be the one that is hard to read.
        for (surface in listOf(true, false)) {
            for (state in listOf(AgentStates.PERMISSION_REQUEST, AgentStates.FAILED)) {
                val fill = Tones.forState(state, surface)!!
                val ratio = Tones.contrastRatio(fill, Tones.onStatus(fill))
                assertTrue(
                    ratio >= 4.5,
                    "$state on a ${if (surface) "dark" else "light"} surface: glyph reads %.2f:1".format(ratio),
                )
            }
        }
    }

    @Test
    fun `subtext has no hex here, because the palette owns it`() {
        assertNull(Tones.of(Tones.SUBTEXT, darkSurface = true))
        assertNull(Tones.of(Tones.SUBTEXT, darkSurface = false))
        assertNull(Tones.forState(AgentStates.EXITED, darkSurface = true))
        // And an unknown state falls into exactly the same hole, rather than
        // being coloured as a fault.
        assertNull(Tones.forState("teleported", darkSurface = true))
    }

    @Test
    fun `every state the runtime publishes resolves to something drawable`() {
        for (state in AgentStates.ALL) {
            val tone = AgentStates.tone(state)
            if (tone == Tone.IDLE) {
                assertNull(Tones.forState(state, true), "$state should defer to the theme's subtext")
            } else {
                assertNotNull(Tones.forState(state, true), "$state has a tone with no colour")
                assertNotNull(Tones.forState(state, false), "$state has no light-surface colour")
            }
        }
    }

    @Test
    fun `the five distinct tones are five distinct colours on both surfaces`() {
        for (surface in listOf(true, false)) {
            val colours = AgentStates.DISTINCT_TONES.map { Tones.of(it.token, surface) }
            assertEquals(
                colours.size,
                colours.toSet().size,
                "two of the five tones share a colour on a ${if (surface) "dark" else "light"} surface",
            )
        }
    }
}
