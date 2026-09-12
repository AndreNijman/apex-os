package com.apexos.remote.core

import org.junit.jupiter.api.Assertions.assertArrayEquals
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.assertThrows

/**
 * `apex-remote-core/src/noise.rs`'s tests, one for one.
 *
 * These use this implementation as both ends, which on its own would prove
 * only self-consistency. `NoiseVectorTest` is what makes them mean something:
 * it pins both halves to bytes `snow` produced, so a handshake that is
 * self-consistent *and* matches those bytes is the handshake the desktop
 * speaks.
 */
class NoiseTest {
    private val v = REMOTE_PROTOCOL_VERSION

    private fun keypair(): InMemoryStaticKey = InMemoryStaticKey.generate()

    /** A completed pairing handshake, as both sides. */
    private fun pairUp(vDevice: Int, vDesktop: Int): Triple<NoiseChannel, NoiseChannel, ByteArray> {
        val desk = keypair()
        val dev = keypair()
        val device = Noise.pairingInitiator(desk.publicKey, vDevice)
        val desktop = Noise.pairingResponder(desk, vDesktop)
        // The device's static key travels in the payload: NK has no slot for an
        // initiator static, and pairing is the moment it becomes known.
        val m1 = device.write(dev.publicKey)
        val got = desktop.read(m1)
        assertArrayEquals(dev.publicKey, got, "the device key did not survive the handshake")
        val m2 = desktop.write("paired".toByteArray())
        assertArrayEquals("paired".toByteArray(), device.read(m2))
        return Triple(device.intoTransport(), desktop.intoTransport(), dev.publicKey)
    }

    @Test
    fun `a pairing handshake completes and the channel carries traffic`() {
        val (device, desktop, _) = pairUp(v, v)
        val request = """{"cmd":"list"}""".toByteArray()
        val sealed = device.seal(request)
        assertFalse(sealed.contentEquals(request), "the channel is not encrypting")
        assertArrayEquals(request, desktop.open(sealed))
        val reply = """{"reply":"sessions"}""".toByteArray()
        assertArrayEquals(reply, device.open(desktop.seal(reply)))
    }

    @Test
    fun `a device that pins the wrong desktop key never completes`() {
        // The property the QR code buys. A man in the middle presenting its own
        // key cannot make the handshake finish against the real desktop, and the
        // device finds out on the first message rather than after sending
        // anything sensitive.
        val desk = keypair()
        val wrong = keypair()
        val device = Noise.pairingInitiator(wrong.publicKey, v)
        val desktop = Noise.pairingResponder(desk, v)
        val m1 = device.write("whatever".toByteArray())
        assertThrows<NoiseException>("a wrong pinned key completed") { desktop.read(m1) }
    }

    @Test
    fun `two ends on different protocol versions do not complete`() {
        // The prologue is bound into the handshake hash, so this fails DURING
        // the handshake rather than after it. A version check done in the first
        // frame instead would be a check the two ends had already agreed a key
        // to disagree about.
        val e = assertThrows<NoiseException> { pairUp(v, v + 1) }
        assertTrue(e.error is NoiseError.Crypto, "${e.error}")
    }

    @Test
    fun `a session handshake tells the desktop which device it is`() {
        // IK's whole reason for being here: the responder learns the initiator's
        // static from the handshake rather than from anything the client
        // asserts, and can then look it up.
        val desk = keypair()
        val dev = keypair()
        val device = Noise.sessionInitiator(dev, desk.publicKey, v)
        val desktop = Noise.sessionResponder(desk, v)
        val m1 = device.write(ByteArray(0))
        // Not in the clear: this is what stops a relay operator correlating a
        // device across connections.
        assertFalse(
            (0..m1.size - 32).any { m1.copyOfRange(it, it + 32).contentEquals(dev.publicKey) },
            "the device's static key is on the wire in plaintext",
        )
        desktop.read(m1)
        assertArrayEquals(dev.publicKey, desktop.remoteStatic, "the responder did not learn the device key")
        device.read(desktop.write(ByteArray(0)))
        assertTrue(device.isHandshakeFinished && desktop.isHandshakeFinished)
        val a = device.intoTransport()
        val b = desktop.intoTransport()
        assertArrayEquals("hello".toByteArray(), b.open(a.seal("hello".toByteArray())))
    }

    @Test
    fun `NK gives the desktop no static key to mistake for an identity`() {
        // The counterpart of the test above, and the reason pairing puts the
        // device key in the payload: a responder that believed it had learned an
        // identity from an NK handshake would have learned nothing.
        val desk = keypair()
        val device = Noise.pairingInitiator(desk.publicKey, v)
        val desktop = Noise.pairingResponder(desk, v)
        desktop.read(device.write(ByteArray(0)))
        assertNull(desktop.remoteStatic)
    }

    @Test
    fun `a tampered message does not decrypt`() {
        val (device, desktop, _) = pairUp(v, v)
        val sealed = device.seal("the original bytes".toByteArray())
        sealed[sealed.size - 1] = (sealed[sealed.size - 1].toInt() xor 0x01).toByte()
        assertThrows<NoiseException>("a flipped bit decrypted") { desktop.open(sealed) }
    }

    @Test
    fun `a replayed message does not decrypt`() {
        // The nonce is the message counter and neither side chooses it, which is
        // what makes the multiplexing above safe: a replayed frame would be a
        // keystroke arriving twice.
        val (device, desktop, _) = pairUp(v, v)
        val first = device.seal("ls -la\r".toByteArray())
        assertArrayEquals("ls -la\r".toByteArray(), desktop.open(first))
        assertThrows<NoiseException>("a replayed frame decrypted") { desktop.open(first) }
    }

    @Test
    fun `a message delivered out of order does not decrypt and does not move the channel on`() {
        // The receiving counter advances only on a SUCCESSFUL decrypt, so a
        // message arriving early is refused and the channel stays where it was.
        // An attacker therefore cannot make the far end skip a frame by
        // injecting a later one — which is the property that matters, and it is
        // stronger than "the channel breaks".
        val (device, desktop, _) = pairUp(v, v)
        val a = device.seal("first".toByteArray())
        val b = device.seal("second".toByteArray())
        assertThrows<NoiseException>("a message from the future decrypted") { desktop.open(b) }
        assertArrayEquals("first".toByteArray(), desktop.open(a), "the refusal moved the channel on")
        assertArrayEquals("second".toByteArray(), desktop.open(b), "and then the next one")
    }

    @Test
    fun `a message too long for noise is refused rather than truncated`() {
        val (device, _, _) = pairUp(v, v)
        val e = assertThrows<NoiseException> { device.seal(ByteArray(Noise.MAX_MESSAGE)) }
        assertTrue(e.error is NoiseError.TooLong, "${e.error}")
        // A frame at the wire layer's own limit does fit, which is the number
        // that constant was chosen to satisfy.
        device.seal(ByteArray(Frame.MAX_PAYLOAD + Frame.HEADER))
    }

    @Test
    fun `a derived public key matches the one the handshake uses`() {
        val key = keypair()
        assertArrayEquals(key.publicKey, Crypto.publicFromSecret(key.exportSecretForSealing()))
    }

    @Test
    fun `a truncated handshake message is refused rather than crashing`() {
        // A phone on a bad connection, and an attacker with a byte to spare,
        // produce the same thing. Neither may reach an array index.
        val desk = keypair()
        val dev = keypair()
        val m1 = Noise.sessionInitiator(dev, desk.publicKey, v).write(ByteArray(0))
        for (cut in 0 until m1.size) {
            val desktop = Noise.sessionResponder(desk, v)
            assertThrows<NoiseException>("a $cut-byte prefix of a handshake was accepted") {
                desktop.read(m1.copyOf(cut))
            }
        }
    }

    @Test
    fun `writing out of turn is refused rather than corrupting the state`() {
        val desk = keypair()
        val device = Noise.pairingInitiator(desk.publicKey, v)
        device.write(ByteArray(0))
        val e = assertThrows<NoiseException> { device.write(ByteArray(0)) }
        assertTrue(e.error is NoiseError.Crypto, "${e.error}")
        assertEquals(false, device.isHandshakeFinished)
    }
}
