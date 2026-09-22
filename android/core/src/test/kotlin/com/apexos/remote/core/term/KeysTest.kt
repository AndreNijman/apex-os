package com.apexos.remote.core.term

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertNotEquals
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

/**
 * P1-055: "normal text input plus Ctrl/Esc/Tab/arrows/function keys and a
 * configurable accessory row."
 *
 * ## Why every one of these is spelled out
 *
 * A key encoded wrongly does not look wrong. It looks like nothing happening,
 * and the person holding the phone concludes the connection is bad. There is
 * no rendering to inspect and no error to read, so the encoding has to be
 * asserted byte for byte against what a terminal actually sends.
 */
class KeysTest {
    private val esc = Ansi.ESC

    private fun enc(key: Key, mods: Mods = Mods.NONE, app: Boolean = false): String =
        Keys.encode(key, mods, app).toString(Charsets.ISO_8859_1)

    @Test
    fun `the arrows change shape when the far end asks for application cursor keys`() {
        // DECCKM is the mode that breaks arrow keys silently. Claude Code,
        // codex and opencode all set it.
        assertEquals("$esc[A", enc(Key.UP))
        assertEquals("${esc}OA", enc(Key.UP, app = true))
        assertEquals("$esc[B", enc(Key.DOWN))
        assertEquals("${esc}OB", enc(Key.DOWN, app = true))
        assertEquals("$esc[C", enc(Key.RIGHT))
        assertEquals("${esc}OC", enc(Key.RIGHT, app = true))
        assertEquals("$esc[D", enc(Key.LEFT))
        assertEquals("${esc}OD", enc(Key.LEFT, app = true))
    }

    @Test
    fun `a modified arrow is CSI even in application mode`() {
        // `ESC O 1 ; 5 A` is not a sequence any terminal emits; xterm switches
        // to the CSI form as soon as there is a modifier to carry.
        assertEquals("$esc[1;5A", enc(Key.UP, Mods(ctrl = true), app = true))
        assertEquals("$esc[1;2D", enc(Key.LEFT, Mods(shift = true)))
        assertEquals("$esc[1;3C", enc(Key.RIGHT, Mods(alt = true)))
        assertEquals("$esc[1;8B", enc(Key.DOWN, Mods(ctrl = true, alt = true, shift = true)))
    }

    @Test
    fun `Home and End follow the same rule as the arrows`() {
        assertEquals("$esc[H", enc(Key.HOME))
        assertEquals("${esc}OH", enc(Key.HOME, app = true))
        assertEquals("$esc[F", enc(Key.END))
        assertEquals("$esc[1;5F", enc(Key.END, Mods(ctrl = true)))
    }

    @Test
    fun `the tilde keys carry their own numbers`() {
        assertEquals("$esc[2~", enc(Key.INSERT))
        assertEquals("$esc[3~", enc(Key.DELETE))
        assertEquals("$esc[5~", enc(Key.PAGE_UP))
        assertEquals("$esc[6~", enc(Key.PAGE_DOWN))
        assertEquals("$esc[6;5~", enc(Key.PAGE_DOWN, Mods(ctrl = true)))
    }

    @Test
    fun `the twelve function keys are the two shapes xterm uses`() {
        // F1..F4 are SS3 letters and F5 upwards are tildes, and the tilde
        // numbers skip 16 and 22. Getting the skips wrong shifts every key
        // above F5 by one.
        assertEquals("${esc}OP", enc(Key.F1))
        assertEquals("${esc}OQ", enc(Key.F2))
        assertEquals("${esc}OR", enc(Key.F3))
        assertEquals("${esc}OS", enc(Key.F4))
        assertEquals("$esc[15~", enc(Key.F5))
        assertEquals("$esc[17~", enc(Key.F6))
        assertEquals("$esc[18~", enc(Key.F7))
        assertEquals("$esc[19~", enc(Key.F8))
        assertEquals("$esc[20~", enc(Key.F9))
        assertEquals("$esc[21~", enc(Key.F10))
        assertEquals("$esc[23~", enc(Key.F11))
        assertEquals("$esc[24~", enc(Key.F12))
        assertEquals("$esc[1;5P", enc(Key.F1, Mods(ctrl = true)))
        assertEquals("$esc[15;2~", enc(Key.F5, Mods(shift = true)))
    }

    @Test
    fun `every function key encodes to something distinct`() {
        // The anti-vacuity guard for the table above: a `when` with a copied
        // arm would give two keys the same bytes, and each assertion would
        // still read plausibly on its own.
        val seen = Key.entries.associateWith { enc(it) }
        assertEquals(Key.entries.size, seen.values.toSet().size, "two keys share an encoding: $seen")
    }

    @Test
    fun `backspace sends DEL and Ctrl-backspace sends BS`() {
        assertEquals("\u007f", enc(Key.BACKSPACE))
        assertEquals("\u0008", enc(Key.BACKSPACE, Mods(ctrl = true)))
        assertEquals("$esc\u007f", enc(Key.BACKSPACE, Mods(alt = true)), "alt-backspace deletes a word")
    }

    @Test
    fun `tab and shift-tab are different keys`() {
        assertEquals("\t", enc(Key.TAB))
        assertEquals("$esc[Z", enc(Key.TAB, Mods(shift = true)))
        assertEquals("$esc\t", enc(Key.TAB, Mods(alt = true)))
    }

    @Test
    fun `escape and enter are one byte each`() {
        assertEquals(esc, enc(Key.ESCAPE))
        assertEquals("\r", enc(Key.ENTER), "a terminal delivers CR for the return key, not LF")
        assertEquals("$esc\r", enc(Key.ENTER, Mods(alt = true)))
    }

    @Test
    fun `Ctrl folds a letter into its control code`() {
        assertEquals("\u0003", Keys.text("c", Mods(ctrl = true)).toString(Charsets.ISO_8859_1), "Ctrl-C")
        assertEquals("\u0003", Keys.text("C", Mods(ctrl = true)).toString(Charsets.ISO_8859_1), "case does not matter")
        assertEquals("\u0012", Keys.text("r", Mods(ctrl = true)).toString(Charsets.ISO_8859_1), "Ctrl-R")
        assertEquals("\u0004", Keys.text("d", Mods(ctrl = true)).toString(Charsets.ISO_8859_1), "Ctrl-D")
    }

    @Test
    fun `the Ctrl cases outside the letters are the ones a naive mask gets wrong`() {
        assertEquals('\u0000', Keys.controlChar(' '), "Ctrl-Space is NUL")
        assertEquals('\u0000', Keys.controlChar('@'))
        assertEquals('\u001b', Keys.controlChar('['), "Ctrl-[ is escape")
        assertEquals('\u001c', Keys.controlChar('\\'))
        assertEquals('\u001d', Keys.controlChar(']'), "Ctrl-] is what agentd uses as a detach key")
        assertEquals('\u001e', Keys.controlChar('^'))
        assertEquals('\u001f', Keys.controlChar('_'))
        assertEquals('\u007f', Keys.controlChar('?'), "Ctrl-? is DEL, not 0x3f masked")
        // A digit has no control code. Turning Ctrl-1 into 0x11 would send
        // Ctrl-Q, which restarts a stopped terminal.
        assertEquals('1', Keys.controlChar('1'))
    }

    @Test
    fun `alt prefixes an escape and does not swallow the character`() {
        assertEquals("${esc}b", Keys.text("b", Mods(alt = true)).toString(Charsets.ISO_8859_1))
        assertEquals("$esc\u0002", Keys.text("b", Mods(alt = true, ctrl = true)).toString(Charsets.ISO_8859_1))
    }

    @Test
    fun `ordinary text goes through unchanged, including outside ASCII`() {
        assertEquals("hello", Keys.text("hello").toString(Charsets.UTF_8))
        assertEquals("héllo→", Keys.text("héllo→").toString(Charsets.UTF_8))
        assertEquals(0, Keys.text("").size)
    }

    // ---- paste ----------------------------------------------------------

    @Test
    fun `a paste is bracketed only when the far end asked for it`() {
        val plain = Keys.paste("ls -la", bracketed = false).toString(Charsets.UTF_8)
        assertEquals("ls -la", plain)
        val bracketed = Keys.paste("ls -la", bracketed = true).toString(Charsets.UTF_8)
        assertEquals("$esc[200~ls -la$esc[201~", bracketed)
    }

    @Test
    fun `a paste cannot end its own bracket`() {
        // Without this, a clipboard containing `ESC[201~rm -rf /` would end
        // the bracket and deliver the rest as if it had been typed — a paste
        // that runs commands, from a clipboard the phone's owner did not
        // necessarily fill.
        val hostile = "safe${esc}[201~rm -rf /"
        val out = Keys.paste(hostile, bracketed = true).toString(Charsets.UTF_8)
        assertEquals(1, countOf(out, "$esc[201~"), "the end marker appears more than once: $out")
        assertTrue(out.endsWith("$esc[201~"))
        assertFalse(out.removeSuffix("$esc[201~").contains("$esc[201~"))
    }

    @Test
    fun `newlines in a paste become carriage returns`() {
        assertEquals("a\rb\rc", Keys.paste("a\nb\r\nc", bracketed = false).toString(Charsets.UTF_8))
    }

    // ---- accessory row ---------------------------------------------------

    @Test
    fun `every default accessory button parses`() {
        for (key in AccessoryKeys.DEFAULT) {
            val action = AccessoryKeys.parse(key.action)
            assertTrue(action != null, "the shipped row has a button nothing can parse: ${key.label}")
        }
    }

    @Test
    fun `an accessory action this build does not know is dropped rather than fatal`() {
        // A row written by a newer version must not stop the app opening.
        assertEquals(null, AccessoryKeys.parse("teleport"))
        assertEquals(null, AccessoryKeys.parse("key:PAUSE_BREAK"))
        assertEquals(null, AccessoryKeys.parse(""))
    }

    @Test
    fun `a modifier button sends nothing and arms the next press`() {
        val ctrl = AccessoryKeys.parse("ctrl")!!
        assertTrue(ctrl is AccessoryAction.Modifier && ctrl.ctrl)
        assertEquals(null, Keys.accessory(ctrl, applicationCursor = false, armed = Mods.NONE))

        val left = AccessoryKeys.parse("key:LEFT")!!
        val bare = Keys.accessory(left, applicationCursor = false, armed = Mods.NONE)!!
        val armed = Keys.accessory(left, applicationCursor = false, armed = Mods(ctrl = true))!!
        assertEquals("$esc[D", bare.toString(Charsets.ISO_8859_1))
        assertEquals("$esc[1;5D", armed.toString(Charsets.ISO_8859_1))
        assertNotEquals(bare.toList(), armed.toList())
    }

    @Test
    fun `an accessory text button respects the armed modifier`() {
        val slash = AccessoryKeys.parse("text:/")!!
        assertEquals("/", Keys.accessory(slash, false, Mods.NONE)!!.toString(Charsets.ISO_8859_1))
        assertEquals("$esc/", Keys.accessory(slash, false, Mods(alt = true))!!.toString(Charsets.ISO_8859_1))
    }

    @Test
    fun `an accessory arrow follows application cursor mode like any other`() {
        val up = AccessoryKeys.parse("key:UP")!!
        assertEquals("$esc[A", Keys.accessory(up, applicationCursor = false, armed = Mods.NONE)!!.toString(Charsets.ISO_8859_1))
        assertEquals("${esc}OA", Keys.accessory(up, applicationCursor = true, armed = Mods.NONE)!!.toString(Charsets.ISO_8859_1))
    }

    private fun countOf(haystack: String, needle: String): Int {
        var n = 0
        var at = haystack.indexOf(needle)
        while (at >= 0) {
            n++
            at = haystack.indexOf(needle, at + 1)
        }
        return n
    }
}
