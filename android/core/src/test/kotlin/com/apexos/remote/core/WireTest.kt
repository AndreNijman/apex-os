package com.apexos.remote.core

import org.junit.jupiter.api.Assertions.assertArrayEquals
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.assertThrows

/**
 * `apex-remote-core/src/wire.rs`'s tests, one for one. The Rust file is the
 * specification; a divergence here is this client misreading a frame the
 * desktop sent, which is the class of bug that is invisible until a terminal
 * shows the wrong bytes.
 */
class WireTest {
    private fun samples(): List<Frame> = listOf(
        Frame.Control("""{"cmd":"list"}""".toByteArray()),
        Frame.Control(ByteArray(0)),
        Frame.Open(1u, """{"cmd":"attach","id":4,"cols":80,"rows":24,"replay":0}""".toByteArray()),
        Frame.Data(7u, byteArrayOf(0x1b, '['.code.toByte(), '2'.code.toByte(), 'K'.code.toByte(), 0x00, 0xff.toByte(), '\n'.code.toByte())),
        Frame.Close(7u, ""),
        Frame.Close(9u, "session exited"),
        Frame.Ping(0L),
        Frame.Pong(-1L), // u64::MAX
    )

    @Test
    fun `every frame round trips`() {
        for (f in samples()) {
            assertEquals(f, Frame.decode(f.encode()), "$f")
        }
    }

    @Test
    fun `a data frame is transparent to arbitrary bytes`() {
        // The property a terminal needs and the reason this is not JSON: every
        // byte value survives, including NUL, 0x1b and invalid UTF-8.
        val all = ByteArray(256) { it.toByte() }
        val back = Frame.decode(Frame.Data(3u, all).encode())
        assertEquals(Frame.Data(3u, all), back)
        assertArrayEquals(all, (back as Frame.Data).bytes)
    }

    @Test
    fun `a control frame cannot carry two requests`() {
        // The framing above this one is line-based, so a payload containing a
        // newline is a second request the far end would parse and act on.
        // Refused in both directions, because a proxy that only checked on the
        // way out would still forward one that arrived.
        val sneaky = "{\"cmd\":\"list\"}\n{\"cmd\":\"decide\",\"id\":1,\"decision\":\"allow\"}".toByteArray()
        val onEncode = assertThrows<WireException> { Frame.Control(sneaky).encode() }
        assertEquals(WireError.Malformed(Frame.TWO_REQUESTS), onEncode.error)
        // And hand-built on the wire, bypassing the encoder entirely.
        val raw = byteArrayOf(Frame.TAG_CONTROL.toByte(), 0, 0, 0, 0) + sneaky
        val onDecode = assertThrows<WireException> { Frame.decode(raw) }
        assertTrue(onDecode.error is WireError.Malformed, "${onDecode.error}")
    }

    @Test
    fun `channel discipline is enforced on the way in`() {
        // A control frame claiming a PTY channel, and terminal bytes claiming
        // the control channel. Both are hand-built: the encoder cannot produce
        // either, which is exactly why the decoder has to check.
        val controlOffChannel =
            byteArrayOf(Frame.TAG_CONTROL.toByte(), 0, 0, 0, 9) + """{"cmd":"list"}""".toByteArray()
        assertEquals(
            WireError.WrongChannel(Frame.TAG_CONTROL, 9u),
            assertThrows<WireException> { Frame.decode(controlOffChannel) }.error,
        )

        val dataOnControl = byteArrayOf(Frame.TAG_DATA.toByte(), 0, 0, 0, 0, 'x'.code.toByte())
        assertEquals(
            WireError.WrongChannel(Frame.TAG_DATA, 0u),
            assertThrows<WireException> { Frame.decode(dataOnControl) }.error,
        )

        for (tag in listOf(Frame.TAG_OPEN, Frame.TAG_CLOSE)) {
            val e = assertThrows<WireException> { Frame.decode(byteArrayOf(tag.toByte(), 0, 0, 0, 0)) }
            assertTrue(e.error is WireError.WrongChannel, "tag $tag was accepted on the control channel")
        }
        for (tag in listOf(Frame.TAG_PING, Frame.TAG_PONG)) {
            val off = byteArrayOf(tag.toByte(), 0, 0, 0, 1) + ByteArray(8) { if (it == 7) 7 else 0 }
            val e = assertThrows<WireException> { Frame.decode(off) }
            assertTrue(e.error is WireError.WrongChannel, "tag $tag was accepted off the control channel")
        }
    }

    @Test
    fun `an unknown tag is an error and not a skip`() {
        for (tag in 6..255) {
            assertEquals(
                WireError.UnknownTag(tag),
                assertThrows<WireException> { Frame.decode(byteArrayOf(tag.toByte(), 0, 0, 0, 0)) }.error,
            )
        }
    }

    @Test
    fun `a truncated frame is short rather than a crash`() {
        for (n in 0 until Frame.HEADER) {
            assertEquals(
                WireError.Short(n),
                assertThrows<WireException> { Frame.decode(ByteArray(n)) }.error,
            )
        }
    }

    @Test
    fun `a ping with the wrong payload length is refused`() {
        // The token is fixed width, so anything else is either a truncated
        // frame or a client that has misread the protocol. Both are errors,
        // and a length check is what makes the second impossible to mistake
        // for a valid token of zero.
        for (len in listOf(0, 1, 7, 9, 16)) {
            val raw = byteArrayOf(Frame.TAG_PING.toByte(), 0, 0, 0, 0) + ByteArray(len)
            val e = assertThrows<WireException> { Frame.decode(raw) }
            assertTrue(e.error is WireError.Malformed, "$len: ${e.error}")
        }
        assertEquals(
            Frame.Ping(0L),
            Frame.decode(byteArrayOf(Frame.TAG_PING.toByte(), 0, 0, 0, 0) + ByteArray(8)),
        )
    }

    @Test
    fun `an over long payload is refused rather than truncated`() {
        val e = assertThrows<WireException> { Frame.Data(1u, ByteArray(Frame.MAX_PAYLOAD + 1)).encode() }
        assertEquals(WireError.TooLong(Frame.MAX_PAYLOAD + 1), e.error)
        // At the limit it encodes, and the encoded frame is exactly what a
        // Noise transport message can hold.
        val bytes = Frame.Data(1u, ByteArray(Frame.MAX_PAYLOAD)).encode()
        assertEquals(Frame.MAX_PAYLOAD + Frame.HEADER, bytes.size)
        assertTrue(bytes.size + 16 <= 65535, "a Noise message cannot hold it")
    }

    @Test
    fun `splitting produces frames that all encode and rejoin`() {
        val payload = ByteArray(Frame.MAX_PAYLOAD * 2 + 13) { (it % 251).toByte() }
        val frames = Frame.dataFrames(5u, payload)
        assertEquals(3, frames.size)
        val rejoined = java.io.ByteArrayOutputStream()
        for (f in frames) {
            val back = Frame.decode(f.encode())
            back as Frame.Data
            assertEquals(5u, back.channel)
            rejoined.write(back.bytes)
        }
        assertArrayEquals(payload, rejoined.toByteArray())
        assertTrue(Frame.dataFrames(5u, ByteArray(0)).isEmpty())
    }

    @Test
    fun `the whole u32 channel space survives a round trip`() {
        // Kotlin has no unsigned primitive on the wire, so the encoder writes
        // a signed Int's bits and the decoder reads them back as UInt. The
        // channel a desktop chose must come back as the channel it chose, and
        // the top of the range is where a sign error shows.
        for (c in listOf(1u, 0x7fffffffu, 0x80000000u, 0xffffffffu)) {
            val back = Frame.decode(Frame.Data(c, byteArrayOf(1)).encode())
            assertEquals(c, (back as Frame.Data).channel, "channel $c")
        }
    }

    @Test
    fun `a frame never prints its payload`() {
        // `toString` on a Data frame reaches logcat and crash reports. A
        // default one would put somebody's terminal in both.
        val secret = "hunter2-in-the-terminal".toByteArray()
        for (f in listOf(Frame.Data(1u, secret), Frame.Control(secret), Frame.Open(2u, secret))) {
            assertTrue("hunter2" !in f.toString(), "${f::class.simpleName} printed its payload")
        }
    }
}
