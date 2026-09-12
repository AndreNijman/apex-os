package com.apexos.remote.core

import org.junit.jupiter.api.Assertions.assertArrayEquals
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.assertThrows
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.EOFException
import java.io.IOException
import java.io.InputStream

/**
 * `apex-remoted/src/net.rs`'s tests, one for one.
 */
class TransportTest {
    @Test
    fun `a message round trips through the length prefix`() {
        for (payload in listOf(ByteArray(0), ByteArray(1), ByteArray(1000) { 7 }, ByteArray(Transport.MAX_MESSAGE) { -1 })) {
            val buf = ByteArrayOutputStream()
            Transport.writeMessage(buf, payload)
            assertEquals(payload.size + 4, buf.size())
            assertArrayEquals(payload, Transport.readMessage(ByteArrayInputStream(buf.toByteArray())))
        }
    }

    @Test
    fun `an announced length beyond the limit is refused before allocating`() {
        // The cheapest denial of service there is: four bytes claiming four
        // gigabytes. This must fail on the header, not on the read that follows
        // it — measured by giving it a stream with nothing after the header at
        // all. On a phone it is also the cheapest way to be killed by the
        // low-memory killer.
        val header = byteArrayOf(-1, -1, -1, -1)
        val e = assertThrows<IOException> { Transport.readMessage(ByteArrayInputStream(header)) }
        assertTrue(e.message!!.contains("the limit is"), e.message!!)
        // And a length just over the limit, which a naive check on the sign bit
        // would let through.
        val justOver = Transport.MAX_MESSAGE + 1
        val over = byteArrayOf(
            (justOver ushr 24).toByte(), (justOver ushr 16).toByte(),
            (justOver ushr 8).toByte(), justOver.toByte(),
        )
        assertThrows<IOException> { Transport.readMessage(ByteArrayInputStream(over)) }
    }

    @Test
    fun `writing an oversized message is refused rather than truncated`() {
        val buf = ByteArrayOutputStream()
        assertThrows<IOException> { Transport.writeMessage(buf, ByteArray(Transport.MAX_MESSAGE + 1)) }
        assertEquals(0, buf.size(), "a refused write still emitted bytes")
    }

    @Test
    fun `a truncated stream is an error and not a short message`() {
        // Two ways a connection dies mid-message, and neither may look like a
        // valid short message to the frame decoder above: a truncated frame
        // handed up would be a keystroke the far end never sent.
        val onlyHeader = byteArrayOf(0, 0, 0, 10) + "abc".toByteArray()
        assertThrows<EOFException> { Transport.readMessage(ByteArrayInputStream(onlyHeader)) }
        assertThrows<EOFException> { Transport.readMessage(ByteArrayInputStream(byteArrayOf(0, 0))) }
        assertThrows<EOFException> { Transport.readMessage(ByteArrayInputStream(ByteArray(0))) }
    }

    @Test
    fun `a message split across many reads is reassembled`() {
        // A phone's TCP stack delivers whatever it has. `read` returning fewer
        // bytes than asked for is the normal case on a slow link, not an
        // error, and a reader that treated it as a complete message would
        // decrypt garbage.
        val payload = ByteArray(5000) { (it % 251).toByte() }
        val buf = ByteArrayOutputStream()
        Transport.writeMessage(buf, payload)
        val dribbling = object : InputStream() {
            private val source = buf.toByteArray()
            private var at = 0
            override fun read(): Int = if (at < source.size) source[at++].toInt() and 0xff else -1
            override fun read(b: ByteArray, off: Int, len: Int): Int {
                if (at >= source.size) return -1
                // One byte at a time, which is the worst a real stream can do.
                b[off] = source[at++]
                return 1
            }
        }
        assertArrayEquals(payload, Transport.readMessage(dribbling))
    }

    @Test
    fun `the hello bytes are the ones apex-remoted reads`() {
        // One plaintext byte before the handshake, and the only plaintext this
        // protocol has after the length prefix. Its values are ASCII letters in
        // `serve.rs`, and a mismatch is a connection the desktop closes with a
        // message about neither a pairing nor a session.
        assertEquals('P'.code.toByte(), Transport.HELLO_PAIR)
        assertEquals('S'.code.toByte(), Transport.HELLO_SESSION)
    }
}
