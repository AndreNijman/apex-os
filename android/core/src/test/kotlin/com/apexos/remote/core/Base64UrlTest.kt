package com.apexos.remote.core

import org.junit.jupiter.api.Assertions.assertArrayEquals
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertNotNull
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Test
import java.util.Base64

/**
 * `apex-remote-core/src/lib.rs`'s base64 tests, one for one.
 */
class Base64UrlTest {
    @Test
    fun `base64 round trips and is url safe`() {
        // The bytes that force the two alphabet differences: `+`/`/` in
        // standard base64, `-`/`_` here.
        val awkward = byteArrayOf(0xfb.toByte(), 0xff.toByte(), 0xbf.toByte(), 0x00, 0x10, 0x83.toByte())
        val text = Base64Url.encode(awkward)
        assertFalse(text.contains('+'), text)
        assertFalse(text.contains('/'), text)
        assertFalse(text.contains('='), text)
        assertArrayEquals(awkward, Base64Url.decode(text))
    }

    @Test
    fun `a nearly valid key does not decode`() {
        val good = Base64Url.encode(ByteArray(32) { 7 })
        assertNotNull(Base64Url.decode(good))
        assertNull(Base64Url.decode("$good="), "padding accepted")
        assertNull(Base64Url.decode("$good!"))
        assertNull(Base64Url.decode("++//"), "standard alphabet accepted")
    }

    @Test
    fun `the jdk decoder would have accepted what the desktop refuses`() {
        // The reason this class exists rather than delegating to the platform.
        // If this assertion ever fails the JDK has become strict and the
        // hand-rolled decoder could be retired — but until then, delegating
        // would make this device accept a key string the desktop rejects.
        val good = Base64Url.encode(ByteArray(32) { 7 })
        val jdk = runCatching { Base64.getUrlDecoder().decode("$good=") }
        assertEquals(
            32,
            jdk.getOrNull()?.size,
            "java.util.Base64 no longer accepts a padded key; Base64Url could delegate",
        )
        assertNull(Base64Url.decode("$good="), "and this decoder still must not")
    }

    @Test
    fun `every byte value round trips at every length`() {
        for (len in 0..40) {
            val bytes = ByteArray(len) { ((it * 37 + 11) % 256).toByte() }
            val text = Base64Url.encode(bytes)
            assertArrayEquals(bytes, Base64Url.decode(text), "length $len")
        }
    }

    @Test
    fun `a non canonical encoding is refused rather than silently accepted`() {
        // "AB" and "AC" would both decode to the single byte 0x00 under a
        // decoder that discarded the leftover bits. Only the canonical one is
        // an encoding of anything, and `data_encoding` on the desktop agrees.
        assertArrayEquals(byteArrayOf(0), Base64Url.decode("AA"))
        assertNull(Base64Url.decode("AB"), "non-canonical trailing bits accepted")
        assertNull(Base64Url.decode("A"), "a lone character is not a group")
    }
}
