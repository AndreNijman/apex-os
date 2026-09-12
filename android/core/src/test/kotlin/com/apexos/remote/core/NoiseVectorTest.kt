package com.apexos.remote.core

import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import org.junit.jupiter.api.Assertions.assertArrayEquals
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Test

/**
 * The test that makes a hand-rolled Noise defensible.
 *
 * Everything else in this module could pass against an implementation that
 * agrees with itself and with nothing else. These replay the exact bytes
 * `snow` — the library `apex-remoted` actually uses — produced from fixed
 * statics and fixed ephemerals, and assert this implementation produces the
 * same ones. Endianness, HKDF, the protocol-name hash, which half of the split
 * is the sending key: a mistake in any of them changes these bytes.
 *
 * The vectors are regenerated and checked on the Rust side by
 * `apex-remote-core`'s `the_committed_android_vectors_are_what_snow_produces`,
 * so neither suite can quietly drift from the other.
 */
class NoiseVectorTest {
    private val version = Vectors.version()
    private val desktopSecret = Vectors.hex("desktop_secret_hex")
    private val desktopPublic = Vectors.hex("desktop_public_hex")
    private val deviceSecret = Vectors.hex("device_secret_hex")
    private val devicePublic = Vectors.hex("device_public_hex")
    private val initiatorEphemeral = EphemeralSource.fixedForTestingOnly(Vectors.hex("initiator_ephemeral_hex"))
    private val responderEphemeral = EphemeralSource.fixedForTestingOnly(Vectors.hex("responder_ephemeral_hex"))

    @Test
    fun `the prologue is the literal bytes apex-remote slash version`() {
        assertArrayEquals(Vectors.hex("prologue_hex"), Noise.prologue(version))
        assertEquals("apex-remote/1", String(Vectors.hex("prologue_hex")))
    }

    @Test
    fun `the fixed keys derive the public halves snow derived`() {
        assertArrayEquals(desktopPublic, Crypto.publicFromSecret(desktopSecret))
        assertArrayEquals(devicePublic, Crypto.publicFromSecret(deviceSecret))
    }

    @Test
    fun `this client's NK pairing message 1 is byte identical to snow's`() {
        val hs = Noise.pairingInitiator(desktopPublic, version, initiatorEphemeral)
        val m1 = hs.write(Vectors.patternHex("nk", "m1_payload_hex"))
        assertEquals(
            Vectors.encodeHex(Vectors.patternHex("nk", "m1_hex")),
            Vectors.encodeHex(m1),
            "the pairing handshake this client sends is not the one the desktop reads",
        )
    }

    @Test
    fun `this client reads snow's NK message 2 and then its transport traffic`() {
        val hs = Noise.pairingInitiator(desktopPublic, version, initiatorEphemeral)
        hs.write(Vectors.patternHex("nk", "m1_payload_hex"))
        val payload = hs.read(Vectors.patternHex("nk", "m2_hex"))
        assertArrayEquals(Vectors.patternHex("nk", "m2_payload_hex"), payload)
        replayTransport("nk", hs.intoTransport())
    }

    @Test
    fun `this client's IK session message 1 is byte identical to snow's`() {
        val hs = Noise.sessionInitiator(
            InMemoryStaticKey(deviceSecret),
            desktopPublic,
            version,
            initiatorEphemeral,
        )
        val m1 = hs.write(Vectors.patternHex("ik", "m1_payload_hex"))
        assertEquals(
            Vectors.encodeHex(Vectors.patternHex("ik", "m1_hex")),
            Vectors.encodeHex(m1),
            "the session handshake this client sends is not the one the desktop reads",
        )
    }

    @Test
    fun `this client reads snow's IK message 2 and then its transport traffic`() {
        val hs = Noise.sessionInitiator(
            InMemoryStaticKey(deviceSecret),
            desktopPublic,
            version,
            initiatorEphemeral,
        )
        hs.write(Vectors.patternHex("ik", "m1_payload_hex"))
        val machine = hs.read(Vectors.patternHex("ik", "m2_hex"))
        assertArrayEquals(Vectors.patternHex("ik", "m2_payload_hex"), machine)
        // Raw bytes and not JSON: `serve.rs` writes `state.machine.as_bytes()`.
        assertEquals("l16", String(machine, Charsets.UTF_8))
        replayTransport("ik", hs.intoTransport())
    }

    @Test
    fun `this client's responder half also matches, so a test can be both ends`() {
        // Not shipped behaviour — the device is always the initiator — but the
        // responder half is what every other test in `NoiseTest` uses as its
        // far end, and a responder that was wrong would make all of them
        // vacuous. Checked against snow's bytes for the same reason.
        val hs = Noise.sessionResponder(InMemoryStaticKey(desktopSecret), version, responderEphemeral)
        val payload = hs.read(Vectors.patternHex("ik", "m1_hex"))
        assertEquals(0, payload.size, "snow's IK message 1 carries an empty payload")
        assertArrayEquals(
            devicePublic,
            hs.remoteStatic,
            "the responder did not learn the device's static key from IK",
        )
        val m2 = hs.write(Vectors.patternHex("ik", "m2_payload_hex"))
        assertEquals(Vectors.encodeHex(Vectors.patternHex("ik", "m2_hex")), Vectors.encodeHex(m2))
    }

    @Test
    fun `the NK responder half matches snow too`() {
        val hs = Noise.pairingResponder(InMemoryStaticKey(desktopSecret), version, responderEphemeral)
        val payload = hs.read(Vectors.patternHex("nk", "m1_hex"))
        assertArrayEquals(Vectors.patternHex("nk", "m1_payload_hex"), payload)
        // NK gives the responder no static to learn: the device's key is in
        // the payload, because pairing is the moment it becomes known.
        assertEquals(null, hs.remoteStatic, "NK handed the responder a static key it cannot have")
        val m2 = hs.write(Vectors.patternHex("nk", "m2_payload_hex"))
        assertEquals(Vectors.encodeHex(Vectors.patternHex("nk", "m2_hex")), Vectors.encodeHex(m2))
    }

    @Test
    fun `the pairing payload snow encrypted is the JSON this client builds`() {
        // The vector's plaintext is a real `PairingRequest` serialised by
        // serde. If this client spelled `user_verification` as
        // `userVerification`, or ordered the fields differently, it would
        // encrypt a different plaintext — and the ciphertext check above would
        // fail with no clue why. This says why.
        val expected = String(Vectors.patternHex("nk", "m1_payload_hex"), Charsets.UTF_8)
        val request = PairingRequest(
            key = Base64Url.encode(devicePublic),
            name = "pixel-8",
            token = Base64Url.encode(ByteArray(Pairing.TOKEN_BYTES) { 7 }),
            userVerification = true,
        )
        assertEquals(expected, Pairing.json.encodeToString(PairingRequest.serializer(), request))
    }

    /** Every recorded transport message, in order, in both directions. */
    private fun replayTransport(which: String, channel: NoiseChannel) {
        for ((i, element) in Vectors.transport(which).withIndex()) {
            val sample = element.jsonObject
            val fromInitiator = sample["from"]!!.jsonPrimitive.content == "initiator"
            val plaintext = Vectors.decodeHex(sample["plaintext_hex"]!!.jsonPrimitive.content)
            val ciphertext = Vectors.decodeHex(sample["ciphertext_hex"]!!.jsonPrimitive.content)
            if (fromInitiator) {
                // The sending counter: message i must seal to exactly what
                // snow sealed, which pins the nonce's advance as well as the key.
                assertEquals(
                    Vectors.encodeHex(ciphertext),
                    Vectors.encodeHex(channel.seal(plaintext)),
                    "$which transport message $i, sent",
                )
            } else {
                assertArrayEquals(
                    plaintext,
                    channel.open(ciphertext),
                    "$which transport message $i, received",
                )
            }
        }
    }
}
