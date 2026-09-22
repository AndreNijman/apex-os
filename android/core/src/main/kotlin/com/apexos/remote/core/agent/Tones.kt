package com.apexos.remote.core.agent

/**
 * What the six state tokens actually look like.
 *
 * ## The second table, and why it is checked rather than trusted
 *
 * [AgentStates] is the state -> token half of the desktop's mapping, and its
 * header says plainly that a third copy of that table is the mistake
 * `agentstate.js` exists to stop. This file is the *other* half — token ->
 * colour — and it is exactly the same risk one layer down: `Colors.qml` picks
 * six hexes per surface lightness, and a phone that picked its own would mean
 * "blocked" was violet on a laptop and whatever looked nice on a phone.
 *
 * So the hexes here are the desktop's, verbatim, and `ToneColoursTest` parses
 * them back out of a vendored copy of `Colors.qml` and fails if the two drift.
 * Both halves are then checked by a test rather than by a comment.
 *
 * ## Why there are two of each
 *
 * `Colors.qml` says it: hue is fixed, lightness is not. `#a6e3a1` is an 11.8:1
 * read on a dark surface and a 1.3:1 read on a light one. The desktop picks
 * the pair with `darkSurface`, which is **not a theme flag** — it is
 * `luminance(background) < 0.18`, computed from the surface actually in use.
 * That distinction is load-bearing on Android in a way it is not on the
 * desktop: Material You hands this app a dynamic scheme whose "dark" surface
 * can be a pale wallpaper-derived tone, and a phone that asked
 * `isSystemInDarkTheme()` instead would put the light-mode hexes on a dark
 * ground or the reverse. [darkSurface] takes the colour.
 *
 * ## `subtext` is not here
 *
 * Five of the six tokens are fixed hues. The sixth, `subtext`, is the
 * palette's own muted foreground — matugen's `on_surface_variant` on the
 * desktop, `colorScheme.onSurfaceVariant` on the phone — and a hex for it here
 * would be a third opinion about the palette. [of] answers `null` for it, and
 * the caller supplies the one its theme already has.
 */
object Tones {
    /** `#RRGGBB` as an `Int`, for a dark surface. */
    val DARK: Map<String, Int> = mapOf(
        "danger" to 0xF87171,
        "warning" to 0xF5C47A,
        "success" to 0xA6E3A1,
        "info" to 0x89B4FA,
        "attention" to 0xD0BCFF,
    )

    /** The same five for a light surface. */
    val LIGHT: Map<String, Int> = mapOf(
        "danger" to 0x8C1D18,
        "warning" to 0x7A4A00,
        "success" to 0x17752F,
        "info" to 0x0B57D0,
        "attention" to 0x6B3FA0,
    )

    /** The token that has no hex, because the palette already owns it. */
    const val SUBTEXT: String = "subtext"

    /**
     * The colour for a token, or `null` when the theme must supply it.
     *
     * `null` for `subtext`, and for any token this build has not been taught —
     * the same pessimism [AgentStates.tone] uses. A caller that got a colour
     * it did not recognise would draw something; a caller that gets `null`
     * falls back to its own foreground, which is right.
     */
    fun of(token: String, darkSurface: Boolean): Int? =
        (if (darkSurface) DARK else LIGHT)[token]

    /** The colour for a state, in one step. `null` means "use the theme's subtext". */
    fun forState(state: String?, darkSurface: Boolean): Int? = of(AgentStates.token(state), darkSurface)

    /**
     * `Colors.qml`'s `darkSurface`: relative luminance below 0.18.
     *
     * The threshold is the desktop's and is not rounded here. 0.18 is well
     * above the midpoint of *perceived* lightness precisely because a surface
     * has to be quite light before a `#f87171` stops reading on it.
     */
    fun darkSurface(background: Int): Boolean = luminance(background) < DARK_SURFACE_BELOW

    const val DARK_SURFACE_BELOW: Double = 0.18

    /** WCAG relative luminance of an `0xRRGGBB`. */
    fun luminance(colour: Int): Double {
        fun channel(v: Int): Double {
            val f = v / 255.0
            return if (f <= 0.04045) f / 12.92 else Math.pow((f + 0.055) / 1.055, 2.4)
        }
        return 0.2126 * channel((colour ushr 16) and 0xFF) +
            0.7152 * channel((colour ushr 8) and 0xFF) +
            0.0722 * channel(colour and 0xFF)
    }

    /** WCAG contrast ratio between two colours. Always >= 1. */
    fun contrastRatio(a: Int, b: Int): Double {
        val ya = luminance(a)
        val yb = luminance(b)
        return if (ya > yb) (ya + 0.05) / (yb + 0.05) else (yb + 0.05) / (ya + 0.05)
    }

    /** The desktop's `fixedLight`. */
    const val FIXED_LIGHT: Int = 0xFFFFFF

    /** The desktop's `fixedDark`. */
    const val FIXED_DARK: Int = 0x1E1E2E

    /**
     * The ink for a glyph sitting **on** a tone fill — a [Weight.SOLID] badge.
     *
     * Whichever of the two fixed foregrounds measures better, and not a
     * lightness threshold. `Colors.qml` states the case against a threshold
     * with a number: at 0.35 it sends white onto `#f87171`, a 2.8:1 read,
     * where the dark foreground next to it would have been 5.9:1. Comparing
     * the two candidates has no seam to place and cannot drift when a token's
     * value changes.
     */
    fun onStatus(fill: Int): Int =
        if (contrastRatio(fill, FIXED_LIGHT) >= contrastRatio(fill, FIXED_DARK)) FIXED_LIGHT else FIXED_DARK

    init {
        check(DARK.keys == LIGHT.keys) { "the two surfaces must carry the same tokens" }
        // Every tone `agentstate.js` names must have a colour or be `subtext`.
        for (tone in Tone.entries) {
            check(tone.token == SUBTEXT || tone.token in DARK) {
                "tone ${tone.name} uses token ${tone.token}, which has no colour"
            }
        }
    }
}
