package com.apexos.remote.ui.theme

import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.ReadOnlyComposable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.toArgb
import com.apexos.remote.core.agent.AgentStates
import com.apexos.remote.core.agent.Tone
import com.apexos.remote.core.agent.Tones
import com.apexos.remote.core.agent.Weight
import com.apexos.remote.core.term.Palette

/**
 * The desktop's status colours, on a phone.
 *
 * Nothing here decides anything. `Tones` in `:core` holds the ten hexes and is
 * checked against a vendored copy of apex-shell's own `Colors.qml`; this file
 * only turns an `Int` into a `Color` and asks the theme for the two things
 * `Tones` deliberately refuses to have an opinion about: the muted foreground
 * `subtext` means, and which of the two surface families is in use.
 *
 * ## `darkSurface` reads the surface, and not `isSystemInDarkTheme()`
 *
 * `Colors.qml` computes it as `luminance(background) < 0.18`, and porting that
 * literally rather than substituting the theme flag matters more on Android
 * than it does on the desktop. Material You hands this app a scheme derived
 * from a wallpaper: a "dark" dynamic scheme can have a surface light enough
 * that `#f87171` on it is unreadable, and a light one can be dim. Asking the
 * colour is right in every case; asking the theme is right in most.
 *
 * The `and 0xFFFFFF` is load-bearing. `Color.toArgb()` returns `0xFFRRGGBB`,
 * and an alpha byte left on the front makes every luminance computation read a
 * number far outside the range it was written for — every surface would come
 * out "light", and the whole palette would flip.
 */
object ApexTones {
    /** The colour for a state, resolved against the scheme in use. */
    @Composable
    @ReadOnlyComposable
    fun forState(state: String?): Color = forToken(AgentStates.token(state))

    /** The colour for a token. `subtext` comes from the palette, as it should. */
    @Composable
    @ReadOnlyComposable
    fun forToken(token: String): Color {
        val scheme = MaterialTheme.colorScheme
        val dark = Tones.darkSurface(scheme.surface.toArgb() and RGB)
        return Tones.of(token, dark)?.let { Color(it or ALPHA) } ?: scheme.onSurfaceVariant
    }

    /**
     * The ink for a glyph sitting on a tone fill.
     *
     * `Tones.onStatus` compares the two fixed foregrounds rather than placing a
     * lightness threshold — `Colors.qml` gives the arithmetic for why — so this
     * is a lookup and not a judgement.
     */
    fun onStatus(fill: Color): Color = Color(Tones.onStatus(fill.toArgb() and RGB) or ALPHA)

    /** How a badge of this weight is filled, given its tone. */
    fun chipFill(weight: Weight, tone: Color): Color = when (weight) {
        Weight.SOLID -> tone
        // `StateBadge.qml`: `Qt.rgba(c.r, c.g, c.b, 0.18)`. The constant lives
        // in `:core` beside the weights it belongs to.
        Weight.TINT -> tone.copy(alpha = Weight.TINT_ALPHA)
        Weight.OUTLINE, Weight.PLAIN -> Color.Transparent
    }

    /** The ink for a badge's glyph. */
    fun chipInk(weight: Weight, tone: Color): Color =
        if (weight == Weight.SOLID) onStatus(tone) else tone

    /** The tone for a state, as the enum rather than a colour. */
    fun toneOf(state: String?): Tone = AgentStates.tone(state)

    fun weightOf(state: String?): Weight = AgentStates.weight(state)

    private const val RGB = 0xFFFFFF
    private const val ALPHA = 0xFF000000.toInt()
}

/**
 * A terminal colour, resolved.
 *
 * The whole SGR decision is `Palette.render` in `:core`, where a test can name
 * what it produced. This is the one line that cannot be tested headlessly, and
 * it is deliberately only that one line.
 */
fun terminalColour(packed: Int): Color = Color(packed or 0xFF000000.toInt())

/** The terminal's own two defaults, from the greeter's palette. */
object TerminalDefaults {
    /** `#cdd6f4`: the greeter's text, and index 15 of the ANSI palette. */
    val Foreground: Int = Palette.ANSI[15]

    /** `#1a282a`: the greeter's ground. */
    const val Background: Int = 0x1A282A
}
