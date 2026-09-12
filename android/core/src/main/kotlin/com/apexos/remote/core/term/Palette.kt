package com.apexos.remote.core.term

/**
 * The sixteen colours [Colour.resolveCube] deliberately refuses to resolve.
 *
 * ## Why this is not in `Colour`
 *
 * `Colour.resolveCube` answers `null` below index 16 and says why: the first
 * sixteen are the *renderer's* idea of "red", and 216 cube entries and 24 greys
 * are arithmetic every terminal agrees on. That split is right, and it leaves a
 * hole — something has to decide what `SGR 31` looks like, and until this file
 * existed nothing did. A renderer that filled the hole itself would put the
 * decision in the Compose layer, where no JVM test can reach it.
 *
 * So the palette lives here, resolution is a pure function of an `Int`, and
 * `:app` only converts a resolved `0xRRGGBB` into a `Color`.
 *
 * ## Which sixteen
 *
 * Catppuccin Mocha's ANSI set, and not for fashion: the APEX palette this app
 * already ships is the greeter's, and the greeter's `#cdd6f4` text and
 * `#1a282a` ground are Catppuccin's own foreground and a darkened Catppuccin
 * base. Picking xterm's original sixteen instead would put a `#ff0000` red and
 * a `#00ff00` green next to a muted teal-grey ground — the combination every
 * screenshot of a terminal from 1995 has, and the reason people install a
 * theme within an hour.
 *
 * The four TUIs this must work with all paint with the first sixteen heavily:
 * Claude Code's prompt frame is index 8, OpenCode's selection bar is 4, Codex's
 * sign-in screen is 6 on 0. So these values are what the app actually looks
 * like, not a fallback nobody reaches.
 *
 * ## Bright is a colour, and it is also sometimes bold
 *
 * A great deal of terminal output sets bold with an ordinary colour and expects
 * the bright one — that is how it worked on hardware where bold *was* the
 * high-intensity bit. [resolve] takes `bold` for exactly this and brightens
 * indices 0..7, which is what xterm, VTE and kitty all do by default. It does
 * NOT brighten 8..15, which are already bright, and it does not touch the cube:
 * an application that asked for `38;5;131` asked for that colour precisely.
 */
object Palette {
    /**
     * The sixteen, as `0xRRGGBB`. Index order is the ANSI one: black, red,
     * green, yellow, blue, magenta, cyan, white, then the same eight bright.
     */
    val ANSI: IntArray = intArrayOf(
        0x45475A, // 0  black    — Catppuccin surface1, not #000000: a true
        //                         black on a #1a282a ground is a hole in the
        //                         screen, and index 0 is what a TUI draws a
        //                         shadow and an inactive border with.
        0xF38BA8, // 1  red
        0xA6E3A1, // 2  green
        0xF9E2AF, // 3  yellow
        0x89B4FA, // 4  blue
        0xF5C2E7, // 5  magenta
        0x94E2D5, // 6  cyan     — the greeter's accent, exactly.
        0xBAC2DE, // 7  white    — subtext1; the *bright* white is the real one.
        0x585B70, // 8  bright black (surface2)
        0xF5A4BB, // 9  bright red
        0xB9E9B5, // 10 bright green
        0xFAE8C0, // 11 bright yellow
        0xA2C4FB, // 12 bright blue
        0xF7CFEC, // 13 bright magenta
        0xABE8DE, // 14 bright cyan
        0xCDD6F4, // 15 bright white — the greeter's text, exactly.
    )

    /**
     * How far the bright six are lifted towards white.
     *
     * ## The defect this constant exists because of
     *
     * Catppuccin's own terminal mapping makes `bright red` **the same hex** as
     * `red`; only black and white differ between its two halves. That is a
     * deliberate choice in a theme whose point is restraint, and shipping it
     * verbatim meant this app rendered ten distinct colours where the
     * criterion says sixteen. It was found by a mutation: removing the
     * bold-brightening rule entirely changed nothing any test could see,
     * because for six of the eight indices there was nothing to see.
     *
     * A TUI that prints warnings in `31` and errors in `91` — and several do,
     * `git` among them — would have shown them identically. So the bright six
     * are lifted 22% towards white: enough to separate them at a glance on a
     * phone, little enough that the palette still looks like one family. The
     * four corners (0, 7, 8, 15) keep their own hand-picked values, because
     * a lifted `surface1` is not `surface2` and a lifted `subtext1` is not the
     * greeter's text.
     *
     * The `init` block below recomputes the six and fails if the literals
     * above have drifted from this rule, so the comment cannot quietly stop
     * being true.
     */
    const val BRIGHT_LIFT: Float = 0.22f

    /** Index 0..7 -> 8..15. Anything else is returned unchanged. */
    fun brighten(index: Int): Int = if (index in 0..7) index + 8 else index

    /**
     * A packed [Colour] resolved to `0xRRGGBB`, or `null` for the default.
     *
     * `null` and not a colour, because "the terminal's own foreground" is a
     * question only the renderer can answer — it depends on the theme, and on
     * whether this cell is inverted. Returning a guess here would be the same
     * mistake `resolveCube` avoided.
     */
    fun resolve(colour: Int, bold: Boolean = false): Int? = when (Colour.kind(colour)) {
        Colour.KIND_DEFAULT -> null
        Colour.KIND_RGB -> (Colour.red(colour) shl 16) or (Colour.green(colour) shl 8) or Colour.blue(colour)
        Colour.KIND_INDEXED -> {
            val index = Colour.index(colour)
            val effective = if (bold) brighten(index) else index
            if (effective < 16) {
                ANSI[effective]
            } else {
                // The cube and the greys, which this file has no opinion about.
                Colour.resolveCube(effective)?.let {
                    (Colour.red(it) shl 16) or (Colour.green(it) shl 8) or Colour.blue(it)
                }
            }
        }
        else -> null
    }

    /**
     * What one cell actually renders as, after inverse, dim and hidden.
     *
     * This is the whole of the SGR-to-pixels decision and it is here rather
     * than in the renderer for one reason: every rule in it is a rule a test
     * can name. A Compose `drawText` call cannot be asserted on headlessly; a
     * pair of ints can.
     *
     * [defaultFg] and [defaultBg] are the theme's, passed in because a light
     * scheme and a dark one disagree about them and this object must not hold
     * either.
     *
     * The order is the one every terminal uses and it matters:
     *
     * 1. resolve both colours, bold brightening the foreground only;
     * 2. **inverse swaps them** — after resolution, so `SGR 7` on a default
     *    cell swaps the theme's two defaults rather than swapping two nulls
     *    and rendering nothing;
     * 3. **dim** blends the foreground halfway to the background — after the
     *    swap, so a dim inverted cell dims the ink actually being drawn;
     * 4. **hidden** makes the foreground the background, last, because it wins
     *    over everything including inverse. A password prompt that leaked its
     *    input because the cell was also inverted would be this rule written
     *    in the wrong order.
     */
    fun render(cell: Cell, defaultFg: Int, defaultBg: Int): Ink {
        val bold = Attrs.has(cell.attrs, Attrs.BOLD)
        var fg = resolve(cell.fg, bold) ?: defaultFg
        var bg = resolve(cell.bg) ?: defaultBg
        if (Attrs.has(cell.attrs, Attrs.INVERSE)) {
            val swap = fg
            fg = bg
            bg = swap
        }
        if (Attrs.has(cell.attrs, Attrs.DIM)) fg = blend(fg, bg, DIM_MIX)
        if (Attrs.has(cell.attrs, Attrs.HIDDEN)) fg = bg
        return Ink(fg, bg)
    }

    /** A cell's two resolved colours, as `0xRRGGBB`. */
    data class Ink(val fg: Int, val bg: Int) {
        override fun toString(): String = "Ink(#%06x on #%06x)".format(fg, bg)
    }

    /** [mix] of [b] into [a], per channel. */
    fun blend(a: Int, b: Int, mix: Float): Int {
        val m = mix.coerceIn(0f, 1f)
        fun channel(shift: Int): Int {
            val x = (a ushr shift) and 0xFF
            val y = (b ushr shift) and 0xFF
            return (x + (y - x) * m).toInt().coerceIn(0, 255)
        }
        return (channel(16) shl 16) or (channel(8) shl 8) or channel(0)
    }

    /**
     * How far `SGR 2` pulls the foreground towards the background.
     *
     * Half, which is what VTE and xterm both do. A dim that went further would
     * make comment text in a diff unreadable on a phone held at arm's length,
     * and every one of these TUIs dims its hints.
     */
    const val DIM_MIX: Float = 0.5f

    init {
        check(ANSI.size == 16) { "the ANSI palette is sixteen colours" }
        for (c in ANSI) check(c in 0..0xFFFFFF) { "#%06x is not a 24-bit colour".format(c) }
        // Sixteen colours means sixteen, not ten with six repeats.
        check(ANSI.toSet().size == 16) { "two ANSI entries are the same colour" }
        // And the bright six really are the normal six lifted, so the comment
        // above cannot rot into a description of numbers nobody derived.
        for (i in 1..6) {
            val expected = blend(ANSI[i], 0xFFFFFF, BRIGHT_LIFT)
            check(ANSI[i + 8] == expected) {
                "bright $i is #%06x and the lift rule gives #%06x".format(ANSI[i + 8], expected)
            }
        }
    }
}
