package com.apexos.remote.core.term

/**
 * A terminal colour, packed into an `Int`.
 *
 * ## Why an int and not a sealed class
 *
 * A phone rendering an 80x24 screen with 2000 lines of scrollback holds about
 * four million cells. A `Colour` object per cell foreground and background
 * would be eight million allocations that the garbage collector then walks,
 * on the device in this system with the least memory and the least patience.
 * So the three cases live in the top byte and the value in the rest, and
 * every operation on them is a static function on a number.
 *
 * ```text
 *  kind  value
 * [0x00][                 0 ]  the terminal's own default
 * [0x01][         0 .. 255  ]  a palette index: 16 ANSI, 216 cube, 24 grey
 * [0x02][ r << 16 | g << 8 | b ]  24-bit, from SGR 38;2;r;g;b
 * ```
 *
 * `DEFAULT` is deliberately zero, so a freshly allocated `IntArray` is
 * already a line of default-coloured cells and clearing one is
 * `java.util.Arrays.fill(line, 0)`.
 */
object Colour {
    /** The terminal's own foreground or background; not a colour, an absence. */
    const val DEFAULT: Int = 0

    const val KIND_DEFAULT: Int = 0
    const val KIND_INDEXED: Int = 1
    const val KIND_RGB: Int = 2

    private const val KIND_SHIFT = 24
    private const val VALUE_MASK = 0x00FFFFFF

    /** One of the 256 palette entries. */
    fun indexed(index: Int): Int {
        require(index in 0..255) { "a palette index is 0..255, not $index" }
        return (KIND_INDEXED shl KIND_SHIFT) or index
    }

    /** A literal 24-bit colour, as `SGR 38;2;r;g;b` names one. */
    fun rgb(r: Int, g: Int, b: Int): Int =
        (KIND_RGB shl KIND_SHIFT) or ((r and 0xFF) shl 16) or ((g and 0xFF) shl 8) or (b and 0xFF)

    fun kind(colour: Int): Int = (colour ushr KIND_SHIFT) and 0xFF

    fun isDefault(colour: Int): Boolean = colour == DEFAULT

    /** The palette index, for an indexed colour. Meaningless otherwise. */
    fun index(colour: Int): Int = colour and 0xFF

    fun red(colour: Int): Int = (colour ushr 16) and 0xFF

    fun green(colour: Int): Int = (colour ushr 8) and 0xFF

    fun blue(colour: Int): Int = colour and 0xFF

    /**
     * A 256-colour index resolved to 24-bit, by the xterm rules.
     *
     * The first sixteen are the palette the *renderer* chooses — a theme's
     * idea of "red" — so they are not resolved here and the caller is told so
     * by getting `null`. The other 240 are arithmetic and identical in every
     * terminal: a 6x6x6 cube at 16, then 24 greys.
     */
    fun resolveCube(index: Int): Int? = when {
        index < 16 -> null
        index < 232 -> {
            val i = index - 16
            rgb(CUBE[i / 36], CUBE[(i / 6) % 6], CUBE[i % 6])
        }
        index < 256 -> {
            val v = 8 + (index - 232) * 10
            rgb(v, v, v)
        }
        else -> null
    }

    /** xterm's six cube levels. Not evenly spaced, and every terminal agrees. */
    private val CUBE = intArrayOf(0, 95, 135, 175, 215, 255)

    /** A readable form, for test failures rather than for the wire. */
    fun describe(colour: Int): String = when (kind(colour)) {
        KIND_DEFAULT -> "default"
        KIND_INDEXED -> "index ${index(colour)}"
        KIND_RGB -> "rgb(${red(colour)},${green(colour)},${blue(colour)})"
        else -> "unknown colour $colour"
    }

    init {
        // The packing only works if a value can never collide with a kind.
        check(VALUE_MASK == 0x00FFFFFF)
    }
}

/**
 * The text attributes a cell can carry, as a bitfield.
 *
 * Same reasoning as [Colour]: one int per cell rather than one object. The
 * values are this file's own, deliberately not the SGR numbers — SGR 1 is
 * bold and SGR 2 is dim and they are not bit 1 and bit 2 of anything.
 */
object Attrs {
    const val NONE: Int = 0
    const val BOLD: Int = 1 shl 0
    const val DIM: Int = 1 shl 1
    const val ITALIC: Int = 1 shl 2
    const val UNDERLINE: Int = 1 shl 3
    const val BLINK: Int = 1 shl 4
    const val INVERSE: Int = 1 shl 5
    const val HIDDEN: Int = 1 shl 6
    const val STRIKE: Int = 1 shl 7

    fun has(attrs: Int, flag: Int): Boolean = (attrs and flag) != 0

    fun describe(attrs: Int): String {
        if (attrs == NONE) return "none"
        val names = ArrayList<String>(8)
        if (has(attrs, BOLD)) names += "bold"
        if (has(attrs, DIM)) names += "dim"
        if (has(attrs, ITALIC)) names += "italic"
        if (has(attrs, UNDERLINE)) names += "underline"
        if (has(attrs, BLINK)) names += "blink"
        if (has(attrs, INVERSE)) names += "inverse"
        if (has(attrs, HIDDEN)) names += "hidden"
        if (has(attrs, STRIKE)) names += "strike"
        return names.joinToString("+")
    }
}

/**
 * One cell, as a value a caller can hold.
 *
 * The screen does not store these — it stores three parallel `IntArray`s —
 * and this exists for the two callers that want one cell at a time: a
 * renderer walking a row, and a test naming what it expects. [Screen.cellAt]
 * builds one on demand.
 */
data class Cell(
    /** A Unicode code point, not a UTF-16 unit. `0x20` for an empty cell. */
    val code: Int,
    val fg: Int = Colour.DEFAULT,
    val bg: Int = Colour.DEFAULT,
    val attrs: Int = Attrs.NONE,
) {
    /** The character run this cell contributes, which is empty for a wide cell's tail. */
    val text: String get() = if (code == WIDE_TAIL) "" else String(Character.toChars(code))

    override fun toString(): String =
        "Cell('${text}', fg=${Colour.describe(fg)}, bg=${Colour.describe(bg)}, ${Attrs.describe(attrs)})"

    companion object {
        /**
         * The code stored in the second column of a double-width character.
         *
         * Not a space: a space would be indistinguishable from a real one when
         * text is copied out, and copying a line of CJK would double its
         * spacing. Zero is never a code point a terminal prints.
         */
        const val WIDE_TAIL: Int = 0
    }
}
