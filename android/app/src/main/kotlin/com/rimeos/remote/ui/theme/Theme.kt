package com.rimeos.remote.ui.theme

import android.app.Activity
import android.os.Build
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.SideEffect
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.view.WindowCompat

/**
 * The Rime design language on a phone.
 *
 * ## Where these colours come from
 *
 * Not invented here. `files/desktop/rime-greet/shell.qml` is the first surface
 * of Rime anybody sees — the login screen — and it is already a deliberate
 * palette: `#1a282a` ground, `#cdd6f4` text, `#94e2d5` for the thing being
 * drawn attention to, `#ff5c5c` for a refusal. Those four are lifted verbatim,
 * so a phone and a laptop are recognisably the same product rather than two
 * things with the same name.
 *
 * ## Dynamic colour is offered and is not the default
 *
 * Material You is available from API 31 and it is genuinely nicer on a phone
 * whose owner has set a wallpaper they like. It is also, by construction, *not*
 * Rime's colours — and this is the app that shows which machine is about to run
 * a command as root. So: the Rime palette is what ships, dynamic colour is a
 * setting, and [RimeRemoteTheme] takes it as a parameter rather than reading a
 * build constant, because the choice belongs to the person holding the phone.
 */
object RimePalette {
    /** The greeter's ground. */
    val Ink = Color(0xFF1A282A)
    /** The greeter's text. */
    val Bone = Color(0xFFCDD6F4)
    /** The greeter's accent: what is being drawn attention to. */
    val Teal = Color(0xFF94E2D5)
    /** The greeter's refusal colour. */
    val Alarm = Color(0xFFFF5C5C)

    /** Derived, and only where a Material scheme needs a slot the greeter has no opinion about. */
    val InkRaised = Color(0xFF223436)
    val InkSunken = Color(0xFF121D1F)
    val TealDeep = Color(0xFF0F5C52)
    val Paper = Color(0xFFF4F7F7)
    val PaperRaised = Color(0xFFFFFFFF)
    val InkOnPaper = Color(0xFF16292B)
}

private val RimeDark = darkColorScheme(
    primary = RimePalette.Teal,
    onPrimary = RimePalette.InkSunken,
    primaryContainer = RimePalette.TealDeep,
    onPrimaryContainer = RimePalette.Bone,
    secondary = RimePalette.Bone,
    onSecondary = RimePalette.InkSunken,
    background = RimePalette.Ink,
    onBackground = RimePalette.Bone,
    surface = RimePalette.Ink,
    onSurface = RimePalette.Bone,
    surfaceVariant = RimePalette.InkRaised,
    onSurfaceVariant = RimePalette.Bone,
    error = RimePalette.Alarm,
    onError = RimePalette.InkSunken,
    outline = RimePalette.TealDeep,
)

private val RimeLight = lightColorScheme(
    primary = RimePalette.TealDeep,
    onPrimary = RimePalette.Paper,
    primaryContainer = RimePalette.Teal,
    onPrimaryContainer = RimePalette.InkOnPaper,
    secondary = RimePalette.InkOnPaper,
    onSecondary = RimePalette.Paper,
    background = RimePalette.Paper,
    onBackground = RimePalette.InkOnPaper,
    surface = RimePalette.Paper,
    onSurface = RimePalette.InkOnPaper,
    surfaceVariant = RimePalette.PaperRaised,
    onSurfaceVariant = RimePalette.InkOnPaper,
    error = Color(0xFFB3261E),
    onError = RimePalette.Paper,
    outline = RimePalette.TealDeep,
)

/**
 * Monospace where the content is machine text.
 *
 * A device key, a machine name and a terminal are all things a human compares
 * character by character, and a proportional font makes `l`, `1` and `I` the
 * same shape. `FontFamily.Monospace` rather than a bundled face: the app is
 * already 41 MB and the platform's monospace is legible everywhere.
 */
val RimeTypography = Typography().let { base ->
    base.copy(
        bodySmall = base.bodySmall.copy(fontFamily = FontFamily.Monospace),
        labelSmall = base.labelSmall.copy(
            fontFamily = FontFamily.Monospace,
            fontWeight = FontWeight.Medium,
            letterSpacing = 0.5.sp,
        ),
    )
}

/** Machine text: a key, an id, a path. Compared by eye, so never proportional. */
val MachineText: TextStyle = TextStyle(fontFamily = FontFamily.Monospace, fontSize = 13.sp)

@Composable
fun RimeRemoteTheme(
    dark: Boolean = isSystemInDarkTheme(),
    useDynamicColour: Boolean = false,
    content: @Composable () -> Unit,
) {
    val context = LocalContext.current
    val scheme = when {
        useDynamicColour && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S ->
            if (dark) dynamicDarkColorScheme(context) else dynamicLightColorScheme(context)
        dark -> RimeDark
        else -> RimeLight
    }
    val view = LocalView.current
    if (!view.isInEditMode) {
        SideEffect {
            val window = (view.context as Activity).window
            // Edge to edge, and the status-bar icons flipped to match the
            // scheme rather than the system's idea of it — a light scheme under
            // a dark system theme otherwise gets white icons on white.
            WindowCompat.getInsetsController(window, view).isAppearanceLightStatusBars = !dark
        }
    }
    MaterialTheme(colorScheme = scheme, typography = RimeTypography, content = content)
}

/**
 * The width of the label column in a key/value row.
 *
 * Three screens lay a label beside a value in a `Row`: the session's telemetry
 * gauges, its detail fields, and the guide's definition blocks. Each wrote the
 * column as `Modifier.width(110.dp)`, and that is the commonest way a screen
 * fails at large text. `dp` is a density unit; it does not move when the owner
 * of the phone raises the system font scale. The label inside the column grows
 * — the box holding it does not — so at a font scale of 2.0 the word "Context"
 * wraps to two lines inside a column sized for one, while the gauge beside it
 * sits where it always did.
 *
 * Scaling the column by the same `fontScale` the text obeys keeps the two
 * growing together. At the default scale of 1.0 this is 110.dp exactly, so
 * nothing moves for a user who has not asked for larger text.
 */
@Composable
fun labelColumnWidth(base: Dp = 110.dp): Dp = base * LocalDensity.current.fontScale
