package com.apexos.remote.core

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.assertThrows

/**
 * `apex-remote-core/src/pairing.rs`'s tests, in the half that is this end's
 * business: reading a QR payload, and refusing the ones that are not offers.
 *
 * The token rules — single use, expiry, constant-time comparison — belong to
 * the desktop, which holds the offer. They are deliberately not reimplemented
 * here: a device that enforced its own copy of them would be enforcing
 * nothing, because the only enforcement that matters happens on the machine
 * being paired with.
 */
class PairingTest {
    private fun offer(
        key: String = Base64Url.encode(ByteArray(32) { 3 }),
        token: String = Base64Url.encode(ByteArray(Pairing.TOKEN_BYTES) { 5 }),
        expiresMs: Long = 1000 + Pairing.OFFER_TTL_MS,
    ) = PairingOffer(
        v = REMOTE_PROTOCOL_VERSION,
        machine = "l16",
        key = key,
        token = token,
        lan = listOf("192.168.1.10:7717"),
        relay = null,
        expiresMs = expiresMs,
    )

    @Test
    fun `an offer round trips through the qr text`() {
        val o = offer()
        val text = Pairing.encodeOffer(o)
        assertTrue(text.startsWith(Pairing.SCHEME), text)
        // URL-safe throughout, because this goes in a URI.
        val body = text.substring(Pairing.SCHEME.length)
        assertTrue(
            body.all { (it in 'A'..'Z') || (it in 'a'..'z') || (it in '0'..'9') || it == '-' || it == '_' },
            text,
        )
        assertEquals(o, Pairing.decodeOffer(text))
    }

    @Test
    fun `a qr payload that is not ours is refused by shape`() {
        for (bad in listOf("https://example.invalid/", "apex-remote", "apex-remote:not base64", "")) {
            val e = assertThrows<PairingException>("$bad was accepted") { Pairing.decodeOffer(bad) }
            assertEquals(PairingError.NotAnOffer, e.error, bad)
        }
        // Ours, and damaged: the scheme is right and the content is not.
        val junk = Pairing.SCHEME + Base64Url.encode("{not an offer".toByteArray())
        assertTrue(assertThrows<PairingException> { Pairing.decodeOffer(junk) }.error is PairingError.Malformed)
    }

    @Test
    fun `an offer whose key or token is the wrong size does not decode`() {
        // Checked at the boundary rather than four layers in, where the error
        // would be about the crypto library.
        val shortKey = offer(key = Base64Url.encode(ByteArray(16) { 1 }))
        assertTrue(
            assertThrows<PairingException> { Pairing.decodeOffer(Pairing.encodeOffer(shortKey)) }
                .error is PairingError.Malformed,
        )
        val shortToken = offer(token = Base64Url.encode(ByteArray(8) { 2 }))
        assertTrue(
            assertThrows<PairingException> { Pairing.decodeOffer(Pairing.encodeOffer(shortToken)) }
                .error is PairingError.Malformed,
        )
    }

    @Test
    fun `an offer knows when it has run out`() {
        val o = offer(expiresMs = Pairing.OFFER_TTL_MS)
        assertTrue(o.isLive(Pairing.OFFER_TTL_MS - 1))
        assertFalse(o.isLive(Pairing.OFFER_TTL_MS))
        assertEquals(1L, o.ttlMs(Pairing.OFFER_TTL_MS - 1))
        assertEquals(0L, o.ttlMs(Pairing.OFFER_TTL_MS + 9999), "a stale offer reported negative time")
    }

    @Test
    fun `a pairing request carries no secret`() {
        // What travels is a public key and a token that is about to be spent.
        // Asserted against a real keypair's secret half rather than a sentinel.
        val identity = InMemoryStaticKey.generate()
        val request = PairingRequest(
            key = identity.publicKeyText(),
            name = "pixel-8",
            token = Base64Url.encode(ByteArray(Pairing.TOKEN_BYTES) { 1 }),
            userVerification = false,
        )
        val text = Pairing.json.encodeToString(PairingRequest.serializer(), request)
        assertFalse(text.contains(Base64Url.encode(identity.exportSecretForSealing())))
    }

    @Test
    fun `an offer decodes the fields a desktop actually emits`() {
        // The JSON `serde` writes for a relayed offer, hand-built rather than
        // round-tripped, so a field renamed on either side shows up here rather
        // than as a pairing that silently loses its relay.
        val key = Base64Url.encode(ByteArray(32) { 3 })
        val token = Base64Url.encode(ByteArray(32) { 5 })
        val json = """{"v":1,"machine":"l16","key":"$key","token":"$token",""" +
            """"lan":["192.168.1.10:7717","[fd00::1]:7717"],""" +
            """"relay":"https://apex-relay.andrenijman.com","expires_ms":1757000000000}"""
        val decoded = Pairing.decodeOffer(Pairing.SCHEME + Base64Url.encode(json.toByteArray()))
        assertEquals("l16", decoded.machine)
        assertEquals(2, decoded.lan.size)
        assertEquals("https://apex-relay.andrenijman.com", decoded.relay)
        assertEquals(1757000000000L, decoded.expiresMs)
    }

    @Test
    fun `an offer with a field this build has never heard of still pairs`() {
        // The desktop will grow fields. A copy of the app already installed will
        // not grow with it, and refusing to pair over an unknown key would mean
        // every desktop change bricked every installed copy.
        val key = Base64Url.encode(ByteArray(32) { 3 })
        val token = Base64Url.encode(ByteArray(32) { 5 })
        val json = """{"v":1,"machine":"l16","key":"$key","token":"$token","expires_ms":1,""" +
            """"something_from_the_future":{"nested":true}}"""
        assertEquals("l16", Pairing.decodeOffer(Pairing.SCHEME + Base64Url.encode(json.toByteArray())).machine)
    }

    @Test
    fun `a device name that would be a display attack is refused here`() {
        // `pairing::complete` on the desktop redeems the token BEFORE it
        // validates the device, so a bad name costs the owner their offer and a
        // walk back to the machine. Refusing it on this side is what stops that.
        val newline = "phone\nAPPROVED"
        val carriage = "phone\rAPPROVED"
        val escape = "phone\u001b[2K"
        for (bad in listOf("", "   ", newline, carriage, escape, "x".repeat(Device.MAX_NAME + 1))) {
            assertThrows<DeviceException>("a bad device name was accepted") { Device.checkName(bad) }
        }
        assertEquals("pixel-8", Device.checkName("  pixel-8  "))
        assertEquals("x".repeat(Device.MAX_NAME), Device.checkName("x".repeat(Device.MAX_NAME)))
    }

    @Test
    fun `the device id is the first sixteen characters of the key`() {
        val text = Vectors.text("device_key_b64")
        assertEquals(text.take(16), Device.idFor(text))
        // And it is what the desktop computed, taken from the vectors rather
        // than from this file's own arithmetic.
        assertEquals(Vectors.text("device_id"), Device.idFor(text))
        assertEquals(Base64Url.encode(Vectors.hex("device_public_hex")), text)
    }

    @Test
    fun `a key that is not thirty two bytes is not a key`() {
        for (bad in listOf("", "!!!", Base64Url.encode(ByteArray(31)), Base64Url.encode(ByteArray(33)))) {
            assertThrows<DeviceException>("a bad key was accepted") { Device.checkKey(bad) }
        }
        Device.checkKey(Vectors.text("desktop_key_b64"))
    }
}
