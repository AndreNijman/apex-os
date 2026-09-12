package com.apexos.remote.core.term

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertNotEquals
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

/**
 * P1-055: "streams real PTY output with ANSI 16/256/truecolor and text
 * attributes."
 *
 * The parsing half of that is `TerminalTest`'s. This is the *rendering* half:
 * `SGR 31` has to become a specific number of pixels-worth of red, and the
 * decision has to be somewhere a test on a machine with no device can reach
 * it. Every assertion below is one a Compose `drawText` call could not be
 * asked to prove.
 */
class PaletteTest {
    private fun cellOf(sgr: String, ch: String = "x"): Cell {
        val t = Terminal(20, 4)
        t.feed(Ansi.csi(sgr) + ch)
        return t.read { it.cellAt(0, 0) }
    }

    private val fg = 0xCDD6F4
    private val bg = 0x1A282A

    @Test
    fun `the sixteen are sixteen distinct entries and all are 24-bit`() {
        assertEquals(16, Palette.ANSI.size)
        for (c in Palette.ANSI) assertTrue(c in 0..0xFFFFFF, "#%06x is not a colour".format(c))
        // Ten was the number before a mutation found it: Catppuccin's own
        // terminal mapping makes bright red the same hex as red, and every
        // test of bold-brightening passed while proving nothing. `ANSI 16` in
        // the criterion means sixteen.
        assertEquals(
            16,
            Palette.ANSI.toSet().size,
            "the palette has repeats: " + Palette.ANSI.withIndex()
                .groupBy({ it.value }, { it.index })
                .filterValues { it.size > 1 }
                .map { (c, at) -> "#%06x at %s".format(c, at) },
        )
    }

    @Test
    fun `every bright entry is lighter than the normal one it pairs with`() {
        // The property that makes `SGR 91` mean something different from
        // `SGR 31` — which is what `git` relies on to separate an error from
        // a warning, and what six identical hexes silently removed.
        for (i in 0..7) {
            val normal = Palette.ANSI[i]
            val bright = Palette.ANSI[i + 8]
            assertNotEquals(normal, bright, "index $i and its bright twin are the same colour")
            assertTrue(
                luminance(bright) > luminance(normal),
                "bright $i (#%06x) is not lighter than $i (#%06x)".format(bright, normal),
            )
        }
    }

    /** WCAG relative luminance, so "lighter" is a measurement rather than a guess. */
    private fun luminance(colour: Int): Double {
        fun channel(v: Int): Double {
            val f = v / 255.0
            return if (f <= 0.04045) f / 12.92 else Math.pow((f + 0.055) / 1.055, 2.4)
        }
        return 0.2126 * channel((colour ushr 16) and 0xFF) +
            0.7152 * channel((colour ushr 8) and 0xFF) +
            0.0722 * channel(colour and 0xFF)
    }

    @Test
    fun `SGR 30 to 37 and 90 to 97 resolve to the palette, in order`() {
        for (i in 0..7) {
            assertEquals(
                Palette.ANSI[i],
                Palette.resolve(cellOf("${30 + i}m").fg),
                "SGR ${30 + i} is not palette entry $i",
            )
            assertEquals(
                Palette.ANSI[i + 8],
                Palette.resolve(cellOf("${90 + i}m").fg),
                "SGR ${90 + i} is not palette entry ${i + 8}",
            )
        }
    }

    @Test
    fun `bold brightens the eight, and does not touch the bright eight or the cube`() {
        // The behaviour every terminal has because bold once WAS the
        // high-intensity bit. A build without it renders Claude Code's bold
        // red diff markers in the dim red.
        assertEquals(Palette.ANSI[9], Palette.resolve(Colour.indexed(1), bold = true))
        assertEquals(Palette.ANSI[1], Palette.resolve(Colour.indexed(1), bold = false))
        // Already bright: unchanged.
        assertEquals(Palette.ANSI[9], Palette.resolve(Colour.indexed(9), bold = true))
        // The cube: an application that said `38;5;131` said exactly that.
        assertEquals(
            Palette.resolve(Colour.indexed(131), bold = false),
            Palette.resolve(Colour.indexed(131), bold = true),
        )
    }

    @Test
    fun `the 216-colour cube and the 24 greys are xterm's arithmetic`() {
        // Index 16 is the cube's black corner, 231 its white one, and 232..255
        // are the greys at 8 + 10n. These are the values every terminal agrees
        // on, so they are checked as literals rather than recomputed.
        assertEquals(0x000000, Palette.resolve(Colour.indexed(16)))
        assertEquals(0xFFFFFF, Palette.resolve(Colour.indexed(231)))
        assertEquals(0x080808, Palette.resolve(Colour.indexed(232)))
        assertEquals(0xEEEEEE, Palette.resolve(Colour.indexed(255)))
        // A mid-cube entry, resolved through a real SGR rather than by hand.
        assertEquals(0xAF5F00, Palette.resolve(cellOf("38;5;130m").fg))
    }

    @Test
    fun `truecolor is carried through unchanged`() {
        assertEquals(0x123456, Palette.resolve(cellOf("38;2;18;52;86m").fg))
        assertEquals(0x654321, Palette.resolve(cellOf("48;2;101;67;33m").bg))
    }

    @Test
    fun `a default colour resolves to null, so the theme decides`() {
        assertNull(Palette.resolve(Colour.DEFAULT))
        val ink = Palette.render(Cell(' '.code), fg, bg)
        assertEquals(fg, ink.fg)
        assertEquals(bg, ink.bg)
    }

    @Test
    fun `inverse swaps the two defaults rather than swapping two absences`() {
        // The bug this is here to stop: swapping before resolving leaves a
        // default-on-default cell with nothing in either slot, and a reverse-
        // video status bar — which every one of these TUIs draws — renders as
        // blank.
        val ink = Palette.render(Cell(' '.code, attrs = Attrs.INVERSE), fg, bg)
        assertEquals(bg, ink.fg)
        assertEquals(fg, ink.bg)
        assertNotEquals(ink.fg, ink.bg)
    }

    @Test
    fun `dim blends towards whatever is actually behind the ink, after the swap`() {
        val plain = Palette.render(Cell('x'.code, fg = Colour.indexed(2)), fg, bg)
        val dim = Palette.render(Cell('x'.code, fg = Colour.indexed(2), attrs = Attrs.DIM), fg, bg)
        assertNotEquals(plain.fg, dim.fg)
        assertEquals(Palette.blend(plain.fg, bg, Palette.DIM_MIX), dim.fg)

        // Inverted AND dim: the ink being dimmed is the one actually drawn,
        // which after the swap is the background colour on the foreground.
        val both = Palette.render(
            Cell('x'.code, fg = Colour.indexed(2), attrs = Attrs.DIM or Attrs.INVERSE),
            fg,
            bg,
        )
        assertEquals(Palette.blend(bg, plain.fg, Palette.DIM_MIX), both.fg)
    }

    @Test
    fun `hidden wins over inverse, because a concealed password must stay concealed`() {
        val ink = Palette.render(
            Cell('s'.code, attrs = Attrs.HIDDEN or Attrs.INVERSE),
            fg,
            bg,
        )
        assertEquals(
            ink.bg,
            ink.fg,
            "a hidden cell drew its character in a colour that is not its background",
        )
    }

    @Test
    fun `bold through a real SGR stream brightens, end to end`() {
        val t = Terminal(20, 4)
        t.feed(Ansi.csi("1;31m") + "E")
        val cell = t.read { it.cellAt(0, 0) }
        assertTrue(Attrs.has(cell.attrs, Attrs.BOLD))
        val ink = Palette.render(cell, fg, bg)
        assertEquals(Palette.ANSI[9], ink.fg)
    }

    @Test
    fun `blend is the identity at its ends`() {
        assertEquals(0x102030, Palette.blend(0x102030, 0xFFFFFF, 0f))
        assertEquals(0xFFFFFF, Palette.blend(0x102030, 0xFFFFFF, 1f))
    }
}
