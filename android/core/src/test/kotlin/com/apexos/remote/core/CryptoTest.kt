package com.apexos.remote.core

import org.junit.jupiter.api.Assertions.assertArrayEquals
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

/**
 * The primitives, against published vectors rather than against themselves.
 *
 * Every key in this protocol comes out of these four functions. A test that
 * only checked they were self-consistent would pass on an implementation that
 * agreed with nothing else in the world, which is precisely the failure mode
 * of writing a handshake by hand.
 */
class CryptoTest {
    @Test
    fun `x25519 matches RFC 7748 section 6 point 1`() {
        // The worked example in the standard: Alice's and Bob's keys, and the
        // shared secret both sides arrive at.
        val alicePrivate = Vectors.decodeHex("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a")
        val alicePublic = Vectors.decodeHex("8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a")
        val bobPrivate = Vectors.decodeHex("5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb")
        val bobPublic = Vectors.decodeHex("de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f")
        val shared = Vectors.decodeHex("4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742")

        assertArrayEquals(alicePublic, Crypto.publicFromSecret(alicePrivate), "Alice's public key")
        assertArrayEquals(bobPublic, Crypto.publicFromSecret(bobPrivate), "Bob's public key")
        assertArrayEquals(shared, Crypto.x25519(alicePrivate, bobPublic), "Alice's view")
        assertArrayEquals(shared, Crypto.x25519(bobPrivate, alicePublic), "Bob's view")
    }

    @Test
    fun `blake2s matches the RFC 7693 appendix A example`() {
        // BLAKE2s-256 of "abc", which is the worked example in the standard.
        assertEquals(
            "508c5e8c327c14e2e1a72ba34eeb452f37458b209ed63a294d999b4c86675982",
            Vectors.encodeHex(Crypto.blake2s("abc".toByteArray())),
        )
    }

    @Test
    fun `sha256 matches its own worked example`() {
        // The rendezvous id depends on this one, and on nothing else.
        assertEquals(
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            Vectors.encodeHex(Crypto.sha256("abc".toByteArray())),
        )
    }

    @Test
    fun `the noise nonce puts the counter in the low bytes little endian`() {
        // The single detail that is invisible to a self-consistent
        // implementation: two ends that both chose big-endian would
        // interoperate with each other and with nothing else.
        assertEquals("000000000000000000000000", Vectors.encodeHex(Crypto.nonce(0)))
        assertEquals("000000000100000000000000", Vectors.encodeHex(Crypto.nonce(1)))
        assertEquals("000000000102000000000000", Vectors.encodeHex(Crypto.nonce(0x0201)))
        assertEquals("00000000ffffffffffffffff", Vectors.encodeHex(Crypto.nonce(-1L)))
    }

    @Test
    fun `hmac over blake2s is keyed and is not the digest of the concatenation`() {
        // The mistake this catches is `blake2s(key + data)`, which is a
        // plausible-looking HMAC that is not one and would produce a whole
        // different key schedule.
        val key = ByteArray(32) { 0x0b }
        val data = "Hi There".toByteArray()
        assertFalse(
            Crypto.hmacBlake2s(key, data).contentEquals(Crypto.blake2s(key, data)),
            "hmac collapsed to a plain digest of key||data",
        )
        // Keyed: a different key gives a different tag, and the same key the same.
        assertArrayEquals(Crypto.hmacBlake2s(key, data), Crypto.hmacBlake2s(key, data))
        assertFalse(
            Crypto.hmacBlake2s(key, data).contentEquals(Crypto.hmacBlake2s(ByteArray(32) { 0x0c }, data)),
        )
    }

    @Test
    fun `chacha20poly1305 detects a flipped bit anywhere`() {
        val key = ByteArray(32) { it.toByte() }
        val nonce = Crypto.nonce(3)
        val ad = "associated".toByteArray()
        val plaintext = "the original bytes".toByteArray()
        val sealed = Crypto.encrypt(key, nonce, ad, plaintext)
        assertEquals(plaintext.size + Crypto.TAGLEN, sealed.size)
        assertArrayEquals(plaintext, Crypto.decrypt(key, nonce, ad, sealed))
        for (i in sealed.indices) {
            val tampered = sealed.copyOf()
            tampered[i] = (tampered[i].toInt() xor 0x01).toByte()
            val opened = runCatching { Crypto.decrypt(key, nonce, ad, tampered) }
            assertTrue(opened.isFailure, "byte $i was flipped and it still decrypted")
        }
        // And the associated data is authenticated too, which is what binds
        // each Noise message to the handshake hash.
        assertTrue(runCatching { Crypto.decrypt(key, nonce, "other".toByteArray(), sealed) }.isFailure)
    }

    @Test
    fun `constant time equals agrees with equality`() {
        assertTrue(Crypto.constantTimeEquals(ByteArray(0), ByteArray(0)))
        assertTrue(Crypto.constantTimeEquals("abc".toByteArray(), "abc".toByteArray()))
        assertFalse(Crypto.constantTimeEquals("abc".toByteArray(), "abd".toByteArray()))
        assertFalse(Crypto.constantTimeEquals("abc".toByteArray(), "ab".toByteArray()))
        assertFalse(Crypto.constantTimeEquals(ByteArray(0), "a".toByteArray()))
        // Differing in the first byte and in the last must both be false — the
        // property a short-circuiting comparison gets right and leaks.
        assertFalse(Crypto.constantTimeEquals(byteArrayOf(0, 98, 99), byteArrayOf(1, 98, 99)))
        assertFalse(Crypto.constantTimeEquals(byteArrayOf(97, 98, 0), byteArrayOf(97, 98, 1)))
    }
}
