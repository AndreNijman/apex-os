package com.apexos.remote.core

import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.File
import org.junit.jupiter.api.Assertions.assertArrayEquals
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertNotEquals
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.assertThrows

/**
 * The relay leg's WebSocket client, against the vectors `relay.rs` is written
 * against.
 *
 * **The point of sharing the vectors rather than writing new ones.** This file
 * and `apexd/apex-remote-core/src/relay.rs` are two implementations of one
 * wire, and they never speak to each other in a unit test: the desktop dials
 * the relay as the host and the phone as the guest, so a round-trip test would
 * be each end agreeing with itself. Every case below is the same case the Rust
 * suite asserts, with the same bytes, so the two are checked against one thing
 * instead of against each other.
 *
 * The frame codec here has a client encoder and a *server* decoder, which is
 * the same asymmetry `relay.rs` has — so [asFromServer] is the one place they
 * are bridged, and it does by hand exactly what a real relay does when it
 * unmasks and forwards.
 */
class RelayTest {

    // ── the relay's own vocabulary ───────────────────────────────────────

    @Test
    fun `the relay vocabulary round trips and has no room for session data`() {
        for (notice in Notice.entries) {
            assertEquals(notice, Notice.parse(notice.frame().toByteArray()))
        }
        // Three words and nothing else. A relay that could attach a payload to
        // a notice would have a channel for saying something about the session
        // it is copying.
        assertEquals("{\"relay\":\"paired\"}", Notice.PAIRED.frame())
    }

    @Test
    fun `a notice this build does not know is ignored rather than fatal`() {
        // A later relay growing a word must not make every older phone drop its
        // connection, so an unknown notice is not an error.
        assertEquals(null, Notice.parse("{\"relay\":\"rebalancing\"}".toByteArray()))
        assertEquals(null, Notice.parse("not json at all".toByteArray()))
        assertEquals(null, Notice.parse("{\"something\":\"else\"}".toByteArray()))
    }

    @Test
    fun `the worker, the desktop and this client spell the wire values the same way`() {
        // Three independently maintained spellings of one wire value is the
        // drift that leaves a phone waiting at a relay that has already paired
        // it, and none of the three would fail a test of its own. So this reads
        // the other two sources and checks them against the enum.
        //
        // Reading the files rather than generating from them: a generated
        // constant is a build step, and a build step nobody runs is a constant
        // that is wrong.
        val room = repoFile("relay/src/room.js").readText()
        val rust = repoFile("apexd/apex-remote-core/src/relay.rs").readText()

        for (notice in Notice.entries) {
            // The JS source writes them in single quotes.
            assertTrue(
                room.contains("'${notice.frame()}'"),
                "the Worker does not send ${notice.frame()} for $notice",
            )
            assertTrue(
                rust.contains("\"${notice.text}\" => Some(Notice::"),
                "apex-remote-core does not parse the notice \"${notice.text}\"",
            )
        }
        // And the two role words, which travel in the query string this client
        // builds and the Worker parses.
        for (role in Rendezvous.Role.entries) {
            assertTrue(room.contains("\"${role.text}\""), "the Worker does not know the role $role")
        }
        // The frame cap is a number both ends allocate against, so it is read
        // out of the Rust source and COMPARED — an assertion that merely found
        // the literal there would pass against any value on this side, which
        // is the defect this whole file exists to avoid.
        val cap = Regex("pub const MAX_FRAME: usize = (\\d+) \\* (\\d+);").find(rust)
        assertTrue(cap != null, "apex-remote-core no longer declares MAX_FRAME where this can read it")
        assertEquals(
            cap!!.groupValues[1].toInt() * cap.groupValues[2].toInt(),
            Relay.MAX_FRAME,
            "the two ends cap a frame at different sizes",
        )
        assertTrue(rust.contains("pub const GUID: &str = \"${Relay.GUID}\""), "the GUID drifted")
    }

    // ── the handshake ────────────────────────────────────────────────────

    @Test
    fun `the accept value is the RFC's own`() {
        // RFC 6455 §1.3, verbatim. The whole handshake is worthless if this is
        // computed with the wrong hash, the wrong alphabet or the wrong
        // constant, and each of those mistakes passes a round-trip test against
        // an implementation that makes the same one.
        assertEquals("s3pPLMBiTxaQ9kYGzzhZRbK+xOo=", Relay.acceptFor("dGhlIHNhbXBsZSBub25jZQ=="))
    }

    @Test
    fun `the accept value is padded standard base64 and not this protocol's alphabet`() {
        // The protocol's own base64 is URL-safe and unpadded, which is right
        // for a QR code and wrong here. A handshake computed with it fails
        // against every real server and against none of the other tests.
        val accept = Relay.acceptFor("dGhlIHNhbXBsZSBub25jZQ==")
        assertTrue(accept.endsWith("="), "padding was stripped: $accept")
        assertTrue(accept.contains("+"), "this vector's accept value contains a non-URL-safe char")
        assertNotEquals(accept, Base64Url.encode(decodeStd(accept)))
    }

    @Test
    fun `the request names the rendezvous, the role and the host`() {
        val e = RelayEndpoint.parse("https://relay.example.com")
        val o = Opening.withNonce(ByteArray(16))
        val text = String(o.request(e, "abc123", Rendezvous.Role.HOST), Charsets.US_ASCII)
        assertTrue(text.startsWith("GET /r/abc123?role=host HTTP/1.1\r\n"), text)
        assertTrue(text.contains("\r\nHost: relay.example.com\r\n"), text)
        assertTrue(text.contains("\r\nUpgrade: websocket\r\n"), text)
        assertTrue(text.contains("\r\nConnection: Upgrade\r\n"), text)
        assertTrue(text.contains("\r\nSec-WebSocket-Version: 13\r\n"), text)
        assertTrue(text.contains("\r\nSec-WebSocket-Key: ${o.key}\r\n"), text)
        assertTrue(text.endsWith("\r\n\r\n"), "the request head does not end")
    }

    @Test
    fun `the key header is sixteen bytes of standard base64 and is not the same twice`() {
        // RFC 6455 §4.1: unpredictable. It is not a secret and authenticates
        // nothing; its job is to make a cached or replayed 101 detectable, and
        // a fixed one makes every 101 replayable.
        val keys = (0 until 32).map { Opening.fresh().key }.toSet()
        assertEquals(32, keys.size, "a repeated nonce in 32 openings")
        assertTrue(keys.all { it.length == 24 && it.endsWith("==") }, "16 bytes is 24 base64 chars")
    }

    @Test
    fun `a guest asks for the other role`() {
        val e = RelayEndpoint.parse("wss://r.example")
        assertEquals("/r/id?role=guest", e.requestTarget("id", Rendezvous.Role.GUEST))
        // The path the app already derives has to be the path the request
        // asks for, or the phone waits at a meeting point of its own.
        assertEquals(
            Rendezvous.path("wss://r.example", "id"),
            e.requestTarget("id", Rendezvous.Role.GUEST),
        )
    }

    @Test
    fun `a 101 is accepted only when the accept value matches`() {
        val o = Opening.withNonce(ByteArray(16) { 3 })
        val good = "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n" +
            "Connection: Upgrade\r\nSec-WebSocket-Accept: ${Relay.acceptFor(o.key)}\r\n\r\n"
        o.check(good.toByteArray())

        // The same response with somebody else's accept value: this is the
        // check that stops a relay replaying one 101 to every client.
        val other = Relay.acceptFor("c29tZWJvZHkgZWxzZSEhIQ==")
        val replayed = "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n" +
            "Connection: Upgrade\r\nSec-WebSocket-Accept: $other\r\n\r\n"
        val e = assertThrows<RelayException> { o.check(replayed.toByteArray()) }
        assertTrue(e.reason is RelayError.Upgrade, "${e.reason}")
    }

    @Test
    fun `anything that is not a 101 upgrade is refused with the relay's own words`() {
        val o = Opening.withNonce(ByteArray(16) { 1 })
        val accept = Relay.acceptFor(o.key)

        // 409 is the status a guest gets when no desktop is waiting at this
        // rendezvous (`relay/src/room.js`), so carrying it through is what
        // tells somebody their computer is off rather than unreachable.
        val conflict = assertThrows<RelayException> {
            o.check("HTTP/1.1 409 Conflict\r\nContent-Length: 0\r\n\r\n".toByteArray())
        }
        assertTrue(conflict.message!!.contains("409"), conflict.message!!)

        for ((what, head) in listOf(
            "no Upgrade" to "HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\n" +
                "Sec-WebSocket-Accept: $accept\r\n\r\n",
            "no Connection" to "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n" +
                "Sec-WebSocket-Accept: $accept\r\n\r\n",
            "no accept value" to "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n" +
                "Connection: Upgrade\r\n\r\n",
            // A relay behind a redirect is a rendezvous somebody else is
            // holding, so it is refused rather than followed.
            "a redirect" to "HTTP/1.1 302 Found\r\nLocation: https://elsewhere.example/\r\n\r\n",
        )) {
            assertThrows<RelayException>("a 101 with $what was accepted") {
                o.check(head.toByteArray())
            }
        }
    }

    @Test
    fun `header names and values are matched the way HTTP defines them`() {
        // A server is free to send `upgrade: WebSocket` and
        // `Connection: keep-alive, Upgrade`. Both are correct HTTP and both
        // break a client that compares strings exactly.
        val o = Opening.withNonce(ByteArray(16) { 9 })
        val head = "HTTP/1.1 101 Switching Protocols\r\nupgrade: WebSocket\r\n" +
            "Connection: keep-alive, Upgrade\r\nSec-WebSocket-Accept: ${Relay.acceptFor(o.key)}\r\n\r\n"
        o.check(head.toByteArray())
    }

    @Test
    fun `the handshake reader stops at the blank line and leaves the first frame`() {
        // The failure this prevents: a buffered reader swallows the first frame
        // with the response head, and the connection hangs waiting for bytes
        // that have already arrived.
        val o = Opening.withNonce(ByteArray(16) { 5 })
        val wire = (
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n" +
                "Connection: Upgrade\r\nSec-WebSocket-Accept: ${Relay.acceptFor(o.key)}\r\n\r\n"
            ).toByteArray() + byteArrayOf(0x82.toByte(), 0x03, 'h'.code.toByte(), 'i'.code.toByte(), '!'.code.toByte())

        val stream = ByteArrayInputStream(wire)
        o.accept(stream)
        assertEquals(WsMessage(WsOp.BINARY, "hi!".toByteArray()), WsReceiver(stream).message())
    }

    @Test
    fun `a server that never answers is a refusal and not a hang`() {
        val o = Opening.withNonce(ByteArray(16) { 7 })
        val e = assertThrows<RelayException> { o.accept(ByteArrayInputStream(ByteArray(0))) }
        assertTrue(e.message!!.contains("without answering"), e.message!!)
    }

    // ── the endpoint ─────────────────────────────────────────────────────

    @Test
    fun `the deployed relay's URL is dialled without being rewritten by hand`() {
        // The real one, on a custom domain rather than workers.dev — see
        // `relay/wrangler.jsonc`. What a person has in front of them is the
        // https URL, and asking them to change the scheme is a step that exists
        // only to be got wrong.
        val e = RelayEndpoint.parse("https://apex-relay.andrenijman.com")
        assertEquals(
            RelayEndpoint(secure = true, host = "apex-relay.andrenijman.com", port = 443, prefix = ""),
            e,
        )
        assertEquals("apex-relay.andrenijman.com", e.authority())
        assertEquals("wss://apex-relay.andrenijman.com", e.toString())
    }

    @Test
    fun `the default port follows the scheme and is left out of the host header`() {
        assertEquals(80, RelayEndpoint.parse("ws://r.example").port)
        assertEquals(443, RelayEndpoint.parse("wss://r.example").port)
        assertEquals(80, RelayEndpoint.parse("http://r.example").port)
        assertEquals(443, RelayEndpoint.parse("https://r.example").port)
        // An explicit non-default port has to appear in Host, or a virtual host
        // on that port answers for the wrong site.
        assertEquals("r.example:8787", RelayEndpoint.parse("ws://r.example:8787").authority())
        // An explicit default port does not.
        assertEquals("r.example", RelayEndpoint.parse("ws://r.example:80").authority())
    }

    @Test
    fun `a path prefix is kept and a trailing slash is not`() {
        val e = RelayEndpoint.parse("https://example.com/apex/")
        assertEquals("/apex", e.prefix)
        assertEquals("/apex/r/xy?role=host", e.requestTarget("xy", Rendezvous.Role.HOST))
        // And the app's own path builder agrees with it, from the same base.
        assertEquals(
            Rendezvous.path("https://example.com/apex/", "xy"),
            e.requestTarget("xy", Rendezvous.Role.GUEST),
        )
    }

    @Test
    fun `an IPv6 literal keeps its brackets in the host header and not in the host`() {
        val e = RelayEndpoint.parse("ws://[2001:db8::1]:8787")
        assertEquals("2001:db8::1", e.host)
        assertEquals(8787, e.port)
        assertEquals("[2001:db8::1]:8787", e.authority())
    }

    @Test
    fun `a relay address that cannot mean what it says is refused before it is dialled`() {
        for (bad in listOf(
            "relay.example.com", // no scheme
            "ftp://relay.example.com", // not a scheme this speaks
            "https://", // no host
            "https://:443", // no host, port only
            "https://r.example:0", // port 0
            "https://r.example:https", // not a number
            "https://r.example:99999", // not a port
            "https://r.example/?role=guest", // a query the client would drop
            "https://r.example/#x", // a fragment the client would drop
            "https://user:pw@r.example", // credentials that end up in logs
            "wss://[2001:db8::1", // an unclosed literal
        )) {
            val e = assertThrows<RelayException>("$bad was accepted") { RelayEndpoint.parse(bad) }
            assertTrue(e.reason is RelayError.Endpoint, "$bad: ${e.reason}")
        }
    }

    // ── the frame codec ──────────────────────────────────────────────────

    @Test
    fun `a client frame is masked and says so`() {
        val out = encodeClient(WsOp.BINARY, "abcde".toByteArray(), byteArrayOf(1, 2, 3, 4))
        assertEquals(0x82.toByte(), out[0], "FIN and the binary opcode")
        assertEquals((0x80 or 5).toByte(), out[1], "the mask bit and the short length")
        assertArrayEquals(byteArrayOf(1, 2, 3, 4), out.copyOfRange(2, 6))
        assertArrayEquals(
            byteArrayOf(
                ('a'.code xor 1).toByte(), ('b'.code xor 2).toByte(), ('c'.code xor 3).toByte(),
                ('d'.code xor 4).toByte(), ('e'.code xor 1).toByte(),
            ),
            out.copyOfRange(6, out.size),
        )
    }

    @Test
    fun `the mask is not the same twice`() {
        // A fixed mask is the mistake that makes the payload recoverable by
        // anyone who can guess four bytes of it, and it passes every round-trip
        // test there is.
        val seen = HashSet<List<Byte>>()
        for (i in 0 until 64) {
            val sink = ByteArrayOutputStream()
            WsSender(sink).binary("the same payload every time".toByteArray())
            seen.add(sink.toByteArray().copyOfRange(2, 6).toList())
        }
        assertTrue(seen.size > 32, "only ${seen.size} distinct masks in 64 frames")
    }

    @Test
    fun `the three length encodings are each used at their boundary`() {
        val short = encodeClient(WsOp.BINARY, ByteArray(125), ByteArray(4))
        assertEquals(125, short[1].toInt() and 0x7f, "125 bytes must use the short form")
        val medium = encodeClient(WsOp.BINARY, ByteArray(126), ByteArray(4))
        assertEquals(126, medium[1].toInt() and 0x7f, "126 bytes must use the 16-bit form")
        assertArrayEquals(byteArrayOf(0, 126), medium.copyOfRange(2, 4))
        val long = encodeClient(WsOp.BINARY, ByteArray(65536), ByteArray(4))
        assertEquals(127, long[1].toInt() and 0x7f, "65536 bytes must use the 64-bit form")
        assertArrayEquals(byteArrayOf(0, 0, 0, 0, 0, 1, 0, 0), long.copyOfRange(2, 10))
    }

    @Test
    fun `a payload survives the wire at every length form`() {
        for (n in listOf(0, 1, 125, 126, 127, 65535, 65536, 70000)) {
            val payload = ByteArray(n) { (it % 251).toByte() }
            val wire = asFromServer(encodeClient(WsOp.BINARY, payload, ByteArray(4) { 0x5a }))
            val got = WsReceiver(ByteArrayInputStream(wire)).message()
            assertEquals(WsOp.BINARY, got.op)
            assertEquals(n, got.payload.size, "length $n did not survive")
            assertArrayEquals(payload, got.payload, "a payload of $n bytes did not survive")
        }
    }

    @Test
    fun `a fragmented message is rejoined in order`() {
        // A relay is free to split a message, and the byte stream carried
        // inside is order-sensitive: a reassembly that dropped or reordered a
        // fragment would corrupt a Noise message and kill the session with a
        // decryption failure that names nothing.
        val wire = byteArrayOf(0x02, 0x03) + "one".toByteArray() +
            byteArrayOf(0x00, 0x03) + "two".toByteArray() +
            byteArrayOf(0x80.toByte(), 0x05) + "three".toByteArray()
        assertEquals(
            WsMessage(WsOp.BINARY, "onetwothree".toByteArray()),
            WsReceiver(ByteArrayInputStream(wire)).message(),
        )
    }

    @Test
    fun `a control frame inside a fragmented message is delivered and not swallowed`() {
        // RFC 6455 §5.4 allows this, and a client that buffered the ping into
        // the message would both corrupt the message and stop answering
        // keepalives — a connection that dies after exactly one long write.
        val wire = byteArrayOf(0x02, 0x03) + "one".toByteArray() +
            byteArrayOf(0x89.toByte(), 0x02) + "pi".toByteArray() +
            byteArrayOf(0x80.toByte(), 0x03) + "two".toByteArray()
        val rx = WsReceiver(ByteArrayInputStream(wire))
        assertEquals(WsMessage(WsOp.PING, "pi".toByteArray()), rx.message())
        assertEquals(WsMessage(WsOp.BINARY, "onetwo".toByteArray()), rx.message())
    }

    @Test
    fun `a frame a server may not send is refused`() {
        for ((what, wire, why) in listOf(
            Triple(
                "a masked frame",
                byteArrayOf(0x82.toByte(), 0x81.toByte(), 1, 2, 3, 4, ('x'.code xor 1).toByte()),
                "a masked frame from the server",
            ),
            Triple("a reserved bit", byteArrayOf(0xc2.toByte(), 0x00), "a reserved bit is set"),
            Triple(
                "an unknown opcode",
                byteArrayOf(0x83.toByte(), 0x00),
                "an opcode this client does not know",
            ),
            Triple(
                "a fragmented control frame",
                byteArrayOf(0x09, 0x00),
                "a fragmented control frame",
            ),
            Triple(
                "a continuation with nothing to continue",
                byteArrayOf(0x80.toByte(), 0x00),
                "a continuation with nothing to continue",
            ),
        )) {
            val e = assertThrows<RelayException>("$what was accepted") {
                WsReceiver(ByteArrayInputStream(wire)).message()
            }
            assertEquals(RelayError.Protocol(why), e.reason, what)
        }
    }

    @Test
    fun `an oversized frame is refused before a byte is allocated`() {
        // The header claims four gigabytes and the connection carries none of
        // them. A client that allocated first would be killed by a two-byte
        // write from the relay — on a phone, by the low-memory killer.
        val fourGiB = 4L shl 30
        val wire = byteArrayOf(0x82.toByte(), 127) + longBytes(fourGiB)
        val e = assertThrows<RelayException> { WsReceiver(ByteArrayInputStream(wire)).message() }
        assertEquals(RelayError.TooLong(fourGiB), e.reason)

        val over = byteArrayOf(0x82.toByte(), 127) + longBytes(Relay.MAX_FRAME + 1L)
        assertEquals(
            RelayError.TooLong(Relay.MAX_FRAME + 1L),
            assertThrows<RelayException> { WsReceiver(ByteArrayInputStream(over)).message() }.reason,
        )
    }

    @Test
    fun `a length with the top bit set is refused and is not a small positive number`() {
        // The mistake a JVM makes that Rust does not: a 64-bit length read into
        // an Int. `0x8000000000000040` truncates to 64, which allocates
        // nothing and then reads sixty-four bytes of a frame that claims eight
        // exabytes — the stream silently desynchronises from there on.
        val wire = byteArrayOf(0x82.toByte(), 127) + longBytes(Long.MIN_VALUE + 64)
        val e = assertThrows<RelayException> { WsReceiver(ByteArrayInputStream(wire)).message() }
        assertTrue(e.reason is RelayError.TooLong, "${e.reason}")
    }

    @Test
    fun `a message fragmented past the cap is refused too`() {
        // Each fragment is legal; the joined message is not. A cap applied only
        // per frame is no cap at all against a hostile relay.
        val chunk = ByteArray(60 * 1024) { 7 }
        val wire = ByteArrayOutputStream()
        wire.write(byteArrayOf(0x02, 126, (chunk.size ushr 8).toByte(), chunk.size.toByte()))
        wire.write(chunk)
        repeat(5) {
            wire.write(byteArrayOf(0x00, 126, (chunk.size ushr 8).toByte(), chunk.size.toByte()))
            wire.write(chunk)
        }
        val e = assertThrows<RelayException> {
            WsReceiver(ByteArrayInputStream(wire.toByteArray())).message()
        }
        assertTrue(e.reason is RelayError.TooLong, "${e.reason}")
    }

    @Test
    fun `a close is sent with a status a relay can log`() {
        val sink = ByteArrayOutputStream()
        WsSender(sink).close()
        val got = WsReceiver(ByteArrayInputStream(asFromServer(sink.toByteArray()))).message()
        assertEquals(WsOp.CLOSE, got.op)
        assertEquals(1000, ((got.payload[0].toInt() and 0xff) shl 8) or (got.payload[1].toInt() and 0xff))
    }

    // ── the seam: a WebSocket presented as the stream pair ───────────────

    @Test
    fun `the carried stream is the same bytes the LAN leg carries`() {
        // The load-bearing claim of the whole design. What goes into
        // `Transport.writeMessage` over a relay link must be, byte for byte,
        // what goes into it over a socket — because nothing above the transport
        // is told which one it is on.
        val messages = listOf(ByteArray(0), byteArrayOf(1), ByteArray(Noise.MAX_MESSAGE) { it.toByte() })

        val overTcp = ByteArrayOutputStream()
        for (m in messages) Transport.writeMessage(overTcp, m)

        val toRelay = ByteArrayOutputStream()
        val link = RelayLink(ByteArrayInputStream(ByteArray(0)), toRelay) {}
        for (m in messages) Transport.writeMessage(link.output, m)

        // Unwrap the frames and the two streams are identical.
        assertArrayEquals(overTcp.toByteArray(), unframe(toRelay.toByteArray()))

        // And read back the other way, through frames a relay chose the size of.
        val back = RelayLink(
            ByteArrayInputStream(serverFrames(overTcp.toByteArray(), chunk = 7)),
            ByteArrayOutputStream(),
        ) {}
        for (m in messages) assertArrayEquals(m, Transport.readMessage(back.input))
    }

    @Test
    fun `one flush is one frame, so a length prefix does not travel alone`() {
        val sink = ByteArrayOutputStream()
        val link = RelayLink(ByteArrayInputStream(ByteArray(0)), sink) {}
        Transport.writeMessage(link.output, ByteArray(10) { 0x42 })
        val frames = frameCount(sink.toByteArray())
        assertEquals(1, frames, "a four-byte length in a frame of its own is a wasted round trip")
    }

    @Test
    fun `a ping is answered with a pong carrying the same bytes`() {
        // A connection that stops answering keepalives is one that dies after
        // exactly as long as the relay's timeout, which is the hardest kind of
        // bug to see from a phone.
        val body = "keepalive".toByteArray()
        val fromRelay = ByteArrayOutputStream()
        fromRelay.write(asFromServer(encodeClient(WsOp.PING, body, ByteArray(4))))
        fromRelay.write(serverFrames("payload".toByteArray(), chunk = 64))

        val toRelay = ByteArrayOutputStream()
        val link = RelayLink(ByteArrayInputStream(fromRelay.toByteArray()), toRelay) {}
        assertArrayEquals("payload".toByteArray(), link.input.readBytes())

        val answer = WsReceiver(ByteArrayInputStream(asFromServer(toRelay.toByteArray()))).message()
        assertEquals(WsMessage(WsOp.PONG, body), answer)
    }

    @Test
    fun `the relay's own words never reach the layer above`() {
        // `waiting`, `paired` and a word this build has never heard of are all
        // consumed here. If any of them reached the carried stream it would be
        // a byte the Noise decoder could not explain, and the session would die
        // with "decryption failed" a long way from the cause.
        val wire = ByteArrayOutputStream()
        wire.write(textFrame(Notice.WAITING.frame()))
        wire.write(serverFrames("abc".toByteArray(), chunk = 64))
        wire.write(textFrame(Notice.PAIRED.frame()))
        wire.write(serverFrames("def".toByteArray(), chunk = 64))
        wire.write(textFrame("{\"relay\":\"rebalancing\"}"))
        wire.write(serverFrames("ghi".toByteArray(), chunk = 64))

        val link = RelayLink(ByteArrayInputStream(wire.toByteArray()), ByteArrayOutputStream()) {}
        assertEquals("abcdefghi", String(link.input.readBytes()))
        assertTrue(link.sawPaired, "the relay said paired and the link did not notice")
    }

    @Test
    fun `nothing arrives after the relay says the far end has gone`() {
        // The mutation this catches: treating `peer-gone` as a word to ignore.
        // The test above would still pass, because its double had nothing left
        // to send — so this one puts bytes AFTER the notice. They are not the
        // desktop's: it has gone. Handing them up would feed the Noise decoder
        // frames from whatever the relay does next.
        val wire = serverFrames("before".toByteArray(), chunk = 64) +
            textFrame(Notice.PEER_GONE.frame()) +
            serverFrames("after the far end was gone".toByteArray(), chunk = 64)
        val link = RelayLink(ByteArrayInputStream(wire), ByteArrayOutputStream()) {}
        assertEquals("before", String(link.input.readBytes()))
        assertEquals(-1, link.input.read(), "the stream did not end when the far end went")
    }

    @Test
    fun `peer-gone and a close are both end of stream and neither names the relay`() {
        // Whatever ends a relayed session has to look exactly like a desktop
        // closing a TCP connection. An exception with the relay's name on it
        // would be the layer above learning which leg it is on — and the whole
        // design is that it cannot.
        for (ending in listOf(
            textFrame(Notice.PEER_GONE.frame()),
            asFromServer(encodeClient(WsOp.CLOSE, byteArrayOf(0x03, 0xe8.toByte()), ByteArray(4))),
            ByteArray(0), // the socket simply died
        )) {
            val wire = serverFrames("half a message".toByteArray(), chunk = 64) + ending
            val link = RelayLink(ByteArrayInputStream(wire), ByteArrayOutputStream()) {}
            assertEquals("half a message", String(link.input.readBytes()))
            assertEquals(-1, link.input.read(), "a relay that went away was not end of stream")
        }
    }

    @Test
    fun `the relay sees ciphertext and the sentinel is nowhere in the frames`() {
        // The property the relay exists to have, asserted rather than promised,
        // and with the same sentinel `apexd/apex-remoted/tests/relay.rs` uses.
        // A real Noise channel is built, the plaintext is written through the
        // link, and what lands in the relay's buffer is scanned by the same
        // encoding-aware search the storage leak tests use.
        val sentinel = "apex-relay-sentinel-4f2a91c7-never-in-the-clear".toByteArray()
        val desktopKey = InMemoryStaticKey.generate()
        val deviceKey = InMemoryStaticKey.generate()
        val device = Noise.pairingInitiator(desktopKey.publicKey, REMOTE_PROTOCOL_VERSION)
        val desktop = Noise.pairingResponder(desktopKey, REMOTE_PROTOCOL_VERSION)
        desktop.read(device.write(deviceKey.publicKey))
        device.read(desktop.write("paired".toByteArray()))
        val deviceChannel = device.intoTransport()
        val desktopChannel = desktop.intoTransport()

        val toRelay = ByteArrayOutputStream()
        val link = RelayLink(ByteArrayInputStream(ByteArray(0)), toRelay) {}
        Transport.writeMessage(link.output, deviceChannel.seal(sentinel))

        val onTheWire = toRelay.toByteArray()
        assertEquals(
            emptyList<String>(),
            Traces.of(sentinel, onTheWire),
            "the sentinel travelled through the relay where its operator could read it",
        )
        // And it really is the sentinel at the other end, so this is not passing
        // because nothing was sent. The desktop reads it off the carried stream
        // with no knowledge that a relay was ever involved.
        assertArrayEquals(
            sentinel,
            desktopChannel.open(Transport.readMessage(ByteArrayInputStream(unframe(onTheWire)))),
        )
    }

    // ── helpers ──────────────────────────────────────────────────────────

    /**
     * Turn client frames back into server frames: same bytes, mask removed.
     *
     * The codec's two halves are a client encoder and a server decoder, so a
     * naive round trip would prove nothing. This is the one place the two are
     * bridged, and it is the unmasking a real relay does.
     */
    private fun asFromServer(clientFrame: ByteArray): ByteArray {
        val short = clientFrame[1].toInt() and 0x7f
        val head = 2 + when (short) {
            126 -> 2
            127 -> 8
            else -> 0
        }
        val mask = clientFrame.copyOfRange(head, head + 4)
        val body = ByteArray(clientFrame.size - head - 4) {
            (clientFrame[head + 4 + it].toInt() xor mask[it % 4].toInt()).toByte()
        }
        val out = clientFrame.copyOfRange(0, head)
        out[1] = (out[1].toInt() and 0x7f).toByte() // the server does not mask
        return out + body
    }

    /** [bytes] cut into unmasked server binary frames of at most [chunk]. */
    private fun serverFrames(bytes: ByteArray, chunk: Int): ByteArray {
        val out = ByteArrayOutputStream()
        var i = 0
        do {
            val n = minOf(chunk, bytes.size - i)
            out.write(asFromServer(encodeClient(WsOp.BINARY, bytes.copyOfRange(i, i + n), ByteArray(4))))
            i += n
        } while (i < bytes.size)
        return out.toByteArray()
    }

    private fun textFrame(text: String): ByteArray =
        asFromServer(encodeClient(WsOp.TEXT, text.toByteArray(), ByteArray(4)))

    /** The carried bytes out of a stream of client frames. */
    private fun unframe(wire: ByteArray): ByteArray {
        val rx = WsReceiver(ByteArrayInputStream(asFromServerAll(wire)))
        val out = ByteArrayOutputStream()
        repeat(frameCount(wire)) { out.write(rx.message().payload) }
        return out.toByteArray()
    }

    private fun asFromServerAll(wire: ByteArray): ByteArray {
        val out = ByteArrayOutputStream()
        for (frame in splitClientFrames(wire)) out.write(asFromServer(frame))
        return out.toByteArray()
    }

    private fun frameCount(wire: ByteArray): Int = splitClientFrames(wire).size

    private fun splitClientFrames(wire: ByteArray): List<ByteArray> {
        val frames = ArrayList<ByteArray>()
        var i = 0
        while (i < wire.size) {
            val short = wire[i + 1].toInt() and 0x7f
            val extra = when (short) {
                126 -> 2
                127 -> 8
                else -> 0
            }
            var length = short
            if (short == 126) {
                length = ((wire[i + 2].toInt() and 0xff) shl 8) or (wire[i + 3].toInt() and 0xff)
            } else if (short == 127) {
                length = 0
                for (k in 4 until 10) length = (length shl 8) or (wire[i + k].toInt() and 0xff)
            }
            val total = 2 + extra + 4 + length
            frames.add(wire.copyOfRange(i, i + total))
            i += total
        }
        return frames
    }

    private fun longBytes(v: Long): ByteArray = ByteArray(8) { (v ushr ((7 - it) * 8)).toByte() }

    private fun decodeStd(text: String): ByteArray {
        val alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
        val body = text.trimEnd('=')
        val out = ByteArrayOutputStream()
        var buffer = 0
        var bits = 0
        for (c in body) {
            buffer = (buffer shl 6) or alphabet.indexOf(c)
            bits += 6
            if (bits >= 8) {
                bits -= 8
                out.write((buffer ushr bits) and 0xff)
            }
        }
        return out.toByteArray()
    }

    private fun repoFile(relative: String): File {
        var dir: File? = File("").absoluteFile
        while (dir != null) {
            val candidate = File(dir, relative)
            if (candidate.isFile) return candidate
            dir = dir.parentFile
        }
        throw AssertionError(
            "$relative was not found from ${File("").absolutePath}. This test compares this " +
                "client's wire values against the other implementations', so a run that could " +
                "not read them must fail rather than report that it found no drift.",
        )
    }
}
