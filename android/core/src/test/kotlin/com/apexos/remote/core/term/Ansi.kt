package com.apexos.remote.core.term

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Test

/**
 * The escape character, defined once for every test in this package.
 *
 * It is here because it was got wrong: a per-file `private val esc = "…"`
 * with the literal character in it is invisible in a diff, invisible in a
 * review, and an empty one makes every assertion in the file test a string
 * with no escape in it — which passes, because a terminal fed `[31mR` prints
 * `[31mR` and nothing asserts that it did not.
 *
 * So: one definition, written as a numeric escape so it is readable, and
 * [AnsiSelfTest] asserts it is the byte it claims to be.
 */
object Ansi {
    const val ESC: String = "\u001b"
    const val BEL: String = "\u0007"

    /** `ESC [ …` */
    fun csi(body: String): String = "$ESC[$body"

    /** `ESC ] … BEL` */
    fun osc(body: String): String = "$ESC]$body$BEL"

    /** `ESC P … ESC \` */
    fun dcs(body: String): String = "${ESC}P$body$ESC\\"
}

/** The guard that makes [Ansi] worth having. */
class AnsiSelfTest {
    @Test
    fun `the escape constant is one byte and it is 0x1b`() {
        assertEquals(1, Ansi.ESC.length, "ESC is not one character; every test using it proves nothing")
        assertEquals(27, Ansi.ESC[0].code)
        assertEquals(1, Ansi.BEL.length)
        assertEquals(7, Ansi.BEL[0].code)
    }

    @Test
    fun `the builders produce the sequences they claim`() {
        assertEquals("\u001b[31m", Ansi.csi("31m"))
        assertEquals("\u001b]0;t\u0007", Ansi.osc("0;t"))
        assertEquals("\u001bP+q544e\u001b\\", Ansi.dcs("+q544e"))
    }
}
