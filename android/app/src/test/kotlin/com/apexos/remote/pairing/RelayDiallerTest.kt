package com.apexos.remote.pairing

import com.apexos.remote.core.Rendezvous
import com.apexos.remote.core.Transport
import com.apexos.remote.core.RelayEndpoint
import com.apexos.remote.core.RelayException
import java.io.ByteArrayOutputStream
import java.io.InputStream
import java.net.ServerSocket
import java.net.Socket
import java.security.MessageDigest
import java.util.Base64
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.TimeUnit
import javax.net.ssl.SSLSocketFactory
import org.junit.jupiter.api.Assertions.assertArrayEquals
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.assertThrows

/**
 * Dialling a relay, against a relay that is not Cloudflare's.
 *
 * The same shape `apexd/apex-remoted/tests/relay.rs` takes and for the same
 * reason: deploying a Worker is a decision about somebody's account, so the
 * relay here is a `ServerSocket` on loopback implementing the room protocol the
 * Worker implements.
 *
 * **The double does not use `Relay.kt`'s codec.** It computes the accept value
 * with `MessageDigest` and `java.util.Base64`, and it builds and parses frames
 * by hand. If it shared the codec, a client that masked wrongly or framed a
 * length wrongly would agree with itself and this file would prove nothing
 * about the wire.
 *
 * What it concludes: that this client speaks the protocol over a real socket,
 * that the carried stream arrives unchanged, and that a refusal carries the
 * relay's own status. What it does not conclude: that Cloudflare's Worker is
 * correct. That needs the deployment, and it is on the card as a device run.
 */
class RelayDiallerTest {

    @Test
    fun `a dialled relay carries the byte stream in both directions`() {
        // The claim under test is the seam itself: what the dialler returns is
        // an InputStream and an OutputStream, and `Transport` — which has never
        // heard of a relay — reads and writes Noise-shaped messages through
        // them exactly as it does through a socket.
        val relay = Double()
        try {
            val dialled = RelayDialler.dial(
                RelayEndpoint.parse("ws://127.0.0.1:${relay.port}"),
                "a-rendezvous-id",
            )
            dialled.link.use { link ->
                val sent = ByteArray(300) { (it % 251).toByte() }
                Transport.writeMessage(link.output, sent)
                // The exact bytes the LAN leg puts on a TCP socket: a
                // big-endian length and then the message. This is the wire
                // contract the whole design rests on, so it is asserted whole
                // rather than by payload.
                assertArrayEquals(
                    frameEnvelope(sent),
                    relay.carried(sent.size + 4),
                    "the relay did not receive the same bytes the LAN leg sends",
                )

                // And back, in frames the relay chose the size of. A client
                // that assumed one frame was one message would fail here, and
                // only here.
                val reply = ByteArray(9000) { (it % 97).toByte() }
                relay.sendCarried(frameEnvelope(reply), chunk = 500)
                assertArrayEquals(reply, Transport.readMessage(link.input))
            }
        } finally {
            relay.close()
        }
    }

    @Test
    fun `the request names the rendezvous and the guest role, on the wire`() {
        val relay = Double()
        try {
            RelayDialler.dial(
                RelayEndpoint.parse("ws://127.0.0.1:${relay.port}/apex"),
                "rv-9",
            ).link.close()
            val head = relay.requestHead()
            assertTrue(head.startsWith("GET /apex/r/rv-9?role=guest HTTP/1.1\r\n"), head)
            assertTrue(head.contains("\r\nHost: 127.0.0.1:${relay.port}\r\n"), head)
            assertTrue(head.contains("\r\nSec-WebSocket-Version: 13\r\n"), head)
        } finally {
            relay.close()
        }
    }

    @Test
    fun `a relay that answers the wrong accept value is refused`() {
        // The check that stops one 101 being replayed to every client. A relay
        // that merely echoes bytes must not be able to look like one that
        // speaks the protocol.
        val relay = Double(acceptValue = { "notTheRightValue=" })
        try {
            val e = assertThrows<RelayException> {
                RelayDialler.dial(RelayEndpoint.parse("ws://127.0.0.1:${relay.port}"), "rv")
            }
            assertTrue(e.message!!.contains("does not match the key this client sent"), e.message!!)
        } finally {
            relay.close()
        }
    }

    @Test
    fun `a 409 from the relay reaches the user with the relay's own status in it`() {
        // 409 is what a guest gets when no desktop is waiting at this
        // rendezvous (`relay/src/room.js`). It is the difference between "your
        // computer is off" and "the relay is down", so it has to survive.
        val relay = Double(status = "409 Conflict", upgrade = false)
        try {
            val e = assertThrows<RelayException> {
                RelayDialler.dial(RelayEndpoint.parse("ws://127.0.0.1:${relay.port}"), "rv")
            }
            assertTrue(e.message!!.contains("409"), e.message!!)
        } finally {
            relay.close()
        }
    }

    @Test
    fun `a failed dial leaves no socket open`() {
        // A revoked phone keeps trying. One leaked descriptor per attempt is a
        // phone that eventually cannot open a file, and the symptom appears
        // nowhere near the cause.
        val relay = Double(status = "409 Conflict", upgrade = false)
        try {
            repeat(20) {
                assertThrows<RelayException> {
                    RelayDialler.dial(RelayEndpoint.parse("ws://127.0.0.1:${relay.port}"), "rv")
                }
            }
            // Every one of those connections was closed from this end, which
            // the double sees as a read returning end-of-stream.
            assertEquals(20, relay.closedByClient, "sockets the client left open")
        } finally {
            relay.close()
        }
    }

    @Test
    fun `a wss relay is dialled with the name checked and not merely the chain`() {
        // **The defect this exists for.** `SSLSocket` validates the certificate
        // chain and then accepts a valid certificate for ANY name unless
        // `endpointIdentificationAlgorithm` is set. Without it this client
        // would hand the rendezvous id — the one value derived from the key a
        // phone pinned — to whoever answered the address, and every test that
        // only asked whether TLS was negotiated would still pass.
        //
        // Asserted on a real SSLSocket rather than through a certificate
        // authority nobody has: what is checked is the configuration the
        // handshake will run under.
        val endpoint = RelayEndpoint.parse("wss://apex-relay.andrenijman.com")
        ServerSocket(0).use { listener ->
            val plain = Socket("127.0.0.1", listener.localPort)
            listener.accept().use {
                plain.use {
                    val tls = RelayDialler.configureTls(
                        plain,
                        endpoint,
                        SSLSocketFactory.getDefault() as SSLSocketFactory,
                    )
                    assertEquals(
                        "HTTPS",
                        tls.sslParameters.endpointIdentificationAlgorithm,
                        "the relay's name would not be checked against its certificate",
                    )
                    // SNI carries the name from the URL, never an address that
                    // was resolved on the way here — Cloudflare answers for the
                    // wrong site without it.
                    assertEquals(
                        listOf("apex-relay.andrenijman.com"),
                        tls.sslParameters.serverNames.map { n -> String(n.encoded) },
                    )
                    assertTrue(
                        tls.enabledProtocols.all { p -> p == "TLSv1.2" || p == "TLSv1.3" },
                        "an obsolete TLS version was left enabled: " +
                            tls.enabledProtocols.joinToString(),
                    )
                }
            }
        }
    }

    @Test
    fun `the path this dials is the path the app already derives`() {
        // Two spellings of one meeting point would leave the phone waiting
        // somewhere the desktop is not, and neither would fail a test of its
        // own.
        val endpoint = RelayEndpoint.parse("https://apex-relay.andrenijman.com/x/")
        assertEquals(
            Rendezvous.path("https://apex-relay.andrenijman.com/x/", "abc"),
            endpoint.requestTarget("abc", Rendezvous.Role.GUEST),
        )
    }

    // ── the double ───────────────────────────────────────────────────────

    /** `[u32 big-endian length][payload]`, which is what `Transport` writes. */
    private fun frameEnvelope(body: ByteArray): ByteArray =
        byteArrayOf(
            (body.size ushr 24).toByte(), (body.size ushr 16).toByte(),
            (body.size ushr 8).toByte(), body.size.toByte(),
        ) + body

    /**
     * A relay, in the shape `relay/src/index.js` has and with none of its code.
     *
     * One connection at a time, which is all any test here opens.
     */
    private class Double(
        private val status: String = "101 Switching Protocols",
        private val upgrade: Boolean = true,
        private val acceptValue: ((String) -> String)? = null,
    ) {
        private val listener = ServerSocket(0)
        private val heads = ArrayBlockingQueue<String>(64)
        private val received = ByteArrayOutputStream()
        private var live: Socket? = null

        @Volatile
        var closedByClient = 0
            private set

        val port: Int get() = listener.localPort

        private val thread = Thread {
            while (!listener.isClosed) {
                val socket = try {
                    listener.accept()
                } catch (e: Exception) {
                    return@Thread
                }
                serve(socket)
            }
        }.also { it.isDaemon = true; it.start() }

        private fun serve(socket: Socket) {
            val input = socket.getInputStream()
            val output = socket.getOutputStream()
            val head = StringBuilder()
            while (!head.endsWith("\r\n\r\n")) {
                val b = input.read()
                if (b < 0) return
                head.append(b.toChar())
            }
            heads.offer(head.toString())
            val key = Regex("(?i)Sec-WebSocket-Key:\\s*(\\S+)").find(head)!!.groupValues[1]
            val accept = acceptValue?.invoke(key) ?: acceptFor(key)
            output.write(
                if (upgrade) {
                    (
                        "HTTP/1.1 $status\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n" +
                            "Sec-WebSocket-Accept: $accept\r\n\r\n"
                        ).toByteArray()
                } else {
                    "HTTP/1.1 $status\r\nContent-Length: 0\r\n\r\n".toByteArray()
                },
            )
            output.flush()
            if (!upgrade) {
                // The client should close this from its end; counting that is
                // how the leak test sees a descriptor it did not get back.
                if (input.read() < 0) closedByClient++
                socket.close()
                return
            }
            // Joined the moment the guest arrives, exactly as the Worker does.
            live = socket
            output.write(serverFrame(0x1, "{\"relay\":\"paired\"}".toByteArray()))
            output.flush()
            try {
                while (true) {
                    val message = readClientFrame(input) ?: break
                    synchronized(received) { received.write(message) }
                }
            } catch (e: Exception) {
                return
            }
        }

        fun requestHead(): String = heads.poll(5, TimeUnit.SECONDS)
            ?: throw AssertionError("the client sent no request")

        /** The carried bytes, once [n] of them have arrived. */
        fun carried(n: Int): ByteArray {
            val deadline = System.currentTimeMillis() + 5_000
            while (System.currentTimeMillis() < deadline) {
                synchronized(received) { if (received.size() >= n) return received.toByteArray() }
                Thread.sleep(5)
            }
            throw AssertionError(
                "the relay received ${synchronized(received) { received.size() }} of $n bytes",
            )
        }

        /** Push [bytes] to the client, cut into frames of at most [chunk]. */
        fun sendCarried(bytes: ByteArray, chunk: Int) {
            val out = live!!.getOutputStream()
            var i = 0
            while (i < bytes.size) {
                val n = minOf(chunk, bytes.size - i)
                out.write(serverFrame(0x2, bytes.copyOfRange(i, i + n)))
                i += n
            }
            out.flush()
        }

        fun close() {
            runCatching { listener.close() }
            runCatching { live?.close() }
            thread.interrupt()
        }

        /** RFC 6455 §1.3, computed independently of the client's. */
        private fun acceptFor(key: String): String =
            Base64.getEncoder().encodeToString(
                MessageDigest.getInstance("SHA-1")
                    .digest((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").toByteArray()),
            )

        /** An unmasked server frame, built by hand. */
        private fun serverFrame(op: Int, payload: ByteArray): ByteArray {
            val out = ByteArrayOutputStream()
            out.write(0x80 or op)
            when {
                payload.size < 126 -> out.write(payload.size)
                payload.size <= 0xffff -> {
                    out.write(126)
                    out.write(payload.size ushr 8)
                    out.write(payload.size and 0xff)
                }
                else -> {
                    out.write(127)
                    for (i in 7 downTo 0) out.write((payload.size ushr (i * 8)) and 0xff)
                }
            }
            out.write(payload)
            return out.toByteArray()
        }

        /** One masked client frame, unmasked — what a real relay does. */
        private fun readClientFrame(input: InputStream): ByteArray? {
            val first = input.read()
            if (first < 0) return null
            val second = input.read()
            if (second < 0) return null
            if (second and 0x80 == 0) throw AssertionError("a client frame was not masked")
            var length = (second and 0x7f).toLong()
            if (length == 126L) {
                length = ((read(input) shl 8) or read(input)).toLong()
            } else if (length == 127L) {
                length = 0
                repeat(8) { length = (length shl 8) or read(input).toLong() }
            }
            val mask = ByteArray(4) { read(input).toByte() }
            val body = ByteArray(length.toInt())
            var got = 0
            while (got < body.size) {
                val n = input.read(body, got, body.size - got)
                if (n < 0) return null
                got += n
            }
            for (i in body.indices) body[i] = (body[i].toInt() xor mask[i % 4].toInt()).toByte()
            // Opcode 8 is a close; anything else that is not binary is not
            // something these tests send.
            return if (first and 0x0f == 0x8) null else body
        }

        private fun read(input: InputStream): Int {
            val b = input.read()
            if (b < 0) throw AssertionError("the client frame ended early")
            return b
        }
    }
}
