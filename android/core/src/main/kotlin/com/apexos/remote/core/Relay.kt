package com.apexos.remote.core

import java.io.Closeable
import java.io.EOFException
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.security.SecureRandom
import org.bouncycastle.crypto.digests.SHA1Digest

/**
 * The relay leg: a WebSocket carrying the bytes the LAN leg carries over TCP.
 *
 * Ported from `apexd/apex-remote-core/src/relay.rs`, which is the
 * specification. Its module docs say why the transport is a WebSocket at all —
 * a machine behind NAT has no inbound port, so both ends dial out and a
 * Cloudflare Worker copies bytes, and a Worker's only long-lived bidirectional
 * transport is a WebSocket.
 *
 * ## The one sentence that has to stay true
 *
 * The relay leg carries, as binary payloads, **exactly** the byte stream the
 * LAN leg carries over TCP: `[u32 big-endian length][Noise ciphertext]`, the
 * framing in [Transport]. Nothing above the transport knows which leg it is
 * on, which is what makes "the relay carries ciphertext it cannot read" a
 * property of the code rather than a promise.
 *
 * That is why [RelayLink] hands out an [InputStream] and an [OutputStream] and
 * nothing else: they are the same pair [Client.pair] and [Client.openSession]
 * take from a [java.net.Socket]. The relay's own vocabulary — [Notice], pings,
 * a close — is consumed here and never surfaces above, so a caller cannot
 * branch on it even by accident. A relay that went away becomes end-of-stream,
 * the same thing a desktop that went away is on the LAN.
 *
 * ## Why this is hand-rolled and there is no OkHttp
 *
 * Four reasons specific to this module, rather than the Rust one's:
 *
 *  1. `:core` is a pure JVM module whose whole dependency list is bouncycastle
 *     and kotlinx-serialization. An HTTP stack in it would be the first, and
 *     its dispatcher threads outlive the unit tests that start them.
 *  2. TLS costs nothing without it: `javax.net.ssl` is the platform trust store
 *     on both a JVM and Android, and everything here is generic over streams so
 *     the caller supplies one — the same position `relay.rs` takes.
 *  3. OkHttp's WebSocket is callback-and-queue shaped, so it would have to be
 *     adapted back into a blocking stream pair to reach the seam above. The
 *     adapter gets written either way; hand-rolling deletes the queue and its
 *     unbounded-buffer policy along with the dependency.
 *  4. The one thing it would buy — redirects, proxies, automatic HTTP/2 — is a
 *     thing this leg must refuse rather than follow. A rendezvous that
 *     redirects is a rendezvous somebody else is holding.
 *
 * What is deliberately **not** here, exactly as in `relay.rs`: TLS, which is
 * the connector's business, and the server half of the handshake, which only a
 * relay needs. [acceptFor] is the one value both sides compute and is public
 * for that reason.
 */
object Relay {
    /**
     * The constant RFC 6455 §1.3 appends to the client's key before hashing.
     *
     * Its only job is to make the accept value impossible to produce by
     * accident, so that a server which merely echoes bytes cannot be mistaken
     * for one that speaks the protocol.
     */
    const val GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"

    /**
     * The largest single frame this client will reassemble.
     *
     * The stream being carried is chunked by whoever is copying it, so a frame
     * size is a relay's choice rather than a protocol constant. A quarter of a
     * megabyte is four times the largest Noise message that can exist and
     * still bounds what one announced length can make this process allocate —
     * which on a phone is also what bounds being killed by the low-memory
     * killer.
     */
    const val MAX_FRAME = 256 * 1024

    /**
     * The value a server must return for a given `Sec-WebSocket-Key`.
     *
     * SHA-1 through bouncycastle's lightweight API rather than
     * `MessageDigest.getInstance`, for the reason [Crypto] gives: on Android
     * the JCA slots are shared with a stripped `BC` and with Conscrypt, and a
     * digest that resolves differently on a phone than on a JVM is a handshake
     * that fails only on the device.
     *
     * Standard padded base64 — RFC 6455's alphabet, not this protocol's
     * URL-safe one. A URL-safe accept value is simply the wrong string.
     */
    fun acceptFor(key: String): String {
        val d = SHA1Digest()
        val k = key.toByteArray(Charsets.US_ASCII)
        d.update(k, 0, k.size)
        d.update(GUID.toByteArray(Charsets.US_ASCII), 0, GUID.length)
        val out = ByteArray(d.digestSize)
        d.doFinal(out, 0)
        return base64Std(out)
    }

    private const val STD = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"

    /**
     * Standard base64, padded.
     *
     * Hand-rolled for the same reason [Base64Url] is — not laxness this time
     * but reach: this is the only place in the protocol that needs the RFC's
     * alphabet, and twenty lines here is cheaper than a second encoder whose
     * behaviour has to be checked on two platforms.
     */
    internal fun base64Std(bytes: ByteArray): String {
        val out = StringBuilder((bytes.size + 2) / 3 * 4)
        var i = 0
        while (i + 3 <= bytes.size) {
            val n = ((bytes[i].toInt() and 0xff) shl 16) or
                ((bytes[i + 1].toInt() and 0xff) shl 8) or
                (bytes[i + 2].toInt() and 0xff)
            out.append(STD[(n ushr 18) and 0x3f])
            out.append(STD[(n ushr 12) and 0x3f])
            out.append(STD[(n ushr 6) and 0x3f])
            out.append(STD[n and 0x3f])
            i += 3
        }
        when (bytes.size - i) {
            1 -> {
                val n = (bytes[i].toInt() and 0xff) shl 16
                out.append(STD[(n ushr 18) and 0x3f])
                out.append(STD[(n ushr 12) and 0x3f])
                out.append("==")
            }
            2 -> {
                val n = ((bytes[i].toInt() and 0xff) shl 16) or ((bytes[i + 1].toInt() and 0xff) shl 8)
                out.append(STD[(n ushr 18) and 0x3f])
                out.append(STD[(n ushr 12) and 0x3f])
                out.append(STD[(n ushr 6) and 0x3f])
                out.append('=')
            }
        }
        return out.toString()
    }

    /** Fresh bytes for a nonce or a frame mask. */
    internal val random = SecureRandom()
}

/** Everything that can go wrong before or during a relay connection. */
sealed class RelayError {
    /** The configured relay URL is not one this client can dial. */
    data class Endpoint(val why: String) : RelayError() {
        override fun toString() = "this relay address cannot be used: $why"
    }

    /** The server answered the upgrade with something other than a switch. */
    data class Upgrade(val why: String) : RelayError() {
        override fun toString() = "the relay refused the connection: $why"
    }

    /** A frame arrived that RFC 6455 does not permit a server to send. */
    data class Protocol(val why: String) : RelayError() {
        override fun toString() = "the relay broke the protocol: $why"
    }

    /** A frame announced more than [Relay.MAX_FRAME] bytes. */
    data class TooLong(val announced: Long) : RelayError() {
        override fun toString() =
            "the relay announced a ${announced}-byte frame; the limit is ${Relay.MAX_FRAME}"
    }
}

/** A relay connection that could not be made, or that broke its own rules. */
class RelayException(val reason: RelayError) : IOException(reason.toString())

/**
 * A relay's address, as it was written in the pairing offer.
 *
 * Parsed once rather than at every reconnect, so that a typo is a refusal with
 * the reason in it and not a connection attempt every few seconds.
 */
data class RelayEndpoint(
    /**
     * Whether the URL asked for TLS. Acted on by the connector and never here
     * — see the module note on what is deliberately absent.
     */
    val secure: Boolean,
    val host: String,
    val port: Int,
    /** Any path the URL carried, without a trailing slash. Usually empty. */
    val prefix: String,
) {
    /** The `Host:` header value: the port is omitted when it is the default. */
    fun authority(): String {
        val h = if (host.contains(':')) "[$host]" else host
        val default = if (secure) 443 else 80
        return if (port == default) h else "$h:$port"
    }

    /**
     * The request target for one end of one rendezvous.
     *
     * The rendezvous id is URL-safe unpadded base64 by construction
     * ([Rendezvous.idFor]), so it needs no escaping here — and a value that
     * would have needed it is a value this client did not derive.
     */
    fun requestTarget(rendezvous: String, role: Rendezvous.Role): String =
        "$prefix/r/$rendezvous?role=${role.text}"

    override fun toString(): String = "${if (secure) "wss" else "ws"}://${authority()}$prefix"

    companion object {
        /**
         * Parse a relay base URL.
         *
         * `https`/`wss` and `http`/`ws` are both accepted and mean the same
         * thing, because the address a person has in front of them is the
         * Worker's `https://` URL and asking anyone to rewrite the scheme is a
         * step that exists only to be got wrong.
         */
        fun parse(base: String): RelayEndpoint {
            val text = base.trim()
            val split = text.indexOf("://")
            if (split < 0) throw RelayException(RelayError.Endpoint("\"$text\" has no scheme"))
            val scheme = text.substring(0, split).lowercase()
            val rest = text.substring(split + 3)
            val secure = when (scheme) {
                "wss", "https" -> true
                "ws", "http" -> false
                else -> throw RelayException(
                    RelayError.Endpoint(
                        "\"$scheme\" is not a scheme this client speaks; use wss, ws, https or http",
                    ),
                )
            }
            // A query or a fragment on the base would be silently dropped when
            // the role is appended, and a relay URL that does not mean what it
            // says is worse than one that is refused.
            for (bad in listOf('?', '#')) {
                if (rest.contains(bad)) {
                    throw RelayException(
                        RelayError.Endpoint(
                            "a relay address may not carry '$bad'; the client appends its own query",
                        ),
                    )
                }
            }
            // Credentials in a URL end up in logs and in the QR code.
            if (rest.substringBefore('/').contains('@')) {
                throw RelayException(
                    RelayError.Endpoint("a relay address may not carry credentials"),
                )
            }

            val slash = rest.indexOf('/')
            val authority = if (slash < 0) rest else rest.substring(0, slash)
            val path = if (slash < 0) "" else rest.substring(slash)
            if (authority.isEmpty()) {
                throw RelayException(RelayError.Endpoint("\"$text\" names no host"))
            }

            // Split host from port, allowing a bracketed IPv6 literal.
            val host: String
            val portText: String?
            if (authority.startsWith("[")) {
                val close = authority.indexOf(']')
                if (close < 0) {
                    throw RelayException(
                        RelayError.Endpoint("\"$authority\" is an unclosed IPv6 literal"),
                    )
                }
                host = authority.substring(1, close)
                val after = authority.substring(close + 1)
                portText = if (after.startsWith(":")) after.substring(1) else null
            } else {
                val colon = authority.lastIndexOf(':')
                if (colon < 0) {
                    host = authority
                    portText = null
                } else {
                    host = authority.substring(0, colon)
                    portText = authority.substring(colon + 1)
                }
            }
            if (host.isEmpty()) {
                throw RelayException(RelayError.Endpoint("\"$text\" names no host"))
            }
            val port = when (portText) {
                null -> if (secure) 443 else 80
                else -> {
                    val n = portText.toIntOrNull()
                    if (n == null || n !in 1..65535) {
                        val why = if (n == 0) "port 0 is not a relay" else "\"$portText\" is not a port"
                        throw RelayException(RelayError.Endpoint(why))
                    }
                    n
                }
            }
            return RelayEndpoint(secure, host, port, path.trimEnd('/'))
        }
    }
}

/**
 * What a relay says about itself, in a text frame.
 *
 * Deliberately tiny and deliberately not about the session. Anything a relay
 * could say about the traffic it is copying is something it should not know.
 */
enum class Notice(val text: String) {
    WAITING("waiting"),
    PAIRED("paired"),
    PEER_GONE("peer-gone"),
    ;

    /** The frame a relay sends, so the parser and the Worker agree. */
    fun frame(): String = "{\"relay\":\"$text\"}"

    companion object {
        /**
         * Parse one text frame.
         *
         * Unknown words are `null` rather than an error: a later relay may
         * grow a notice this build does not have, and a client that killed the
         * connection over one would make every relay upgrade a flag day.
         *
         * Read with a small hand parser rather than the JSON codec because
         * this runs on the read path of every relayed connection and the whole
         * grammar is one key with one string value.
         */
        fun parse(bytes: ByteArray): Notice? {
            val text = bytes.toString(Charsets.UTF_8)
            val key = text.indexOf("\"relay\"")
            if (key < 0) return null
            val colon = text.indexOf(':', key)
            if (colon < 0) return null
            val open = text.indexOf('"', colon)
            if (open < 0) return null
            val close = text.indexOf('"', open + 1)
            if (close < 0) return null
            val word = text.substring(open + 1, close)
            return entries.firstOrNull { it.text == word }
        }
    }
}

/** The frame opcodes this client handles. */
enum class WsOp(val code: Int) {
    CONTINUATION(0x0),
    TEXT(0x1),
    BINARY(0x2),
    CLOSE(0x8),
    PING(0x9),
    PONG(0xa),
    ;

    fun isControl(): Boolean = this == CLOSE || this == PING || this == PONG

    companion object {
        fun fromCode(code: Int): WsOp? = entries.firstOrNull { it.code == code }
    }
}

/** One complete message, after any fragments have been joined. */
data class WsMessage(val op: WsOp, val payload: ByteArray) {
    // Generated equals/hashCode compare a ByteArray by identity, which would
    // make every assertion in the tests below pass against the wrong bytes.
    override fun equals(other: Any?): Boolean =
        other is WsMessage && other.op == op && other.payload.contentEquals(payload)

    override fun hashCode(): Int = 31 * op.hashCode() + payload.contentHashCode()

    override fun toString(): String = "WsMessage($op, ${payload.size} bytes)"
}

/**
 * Encode one frame as a client sends it: always masked, per RFC 6455 §5.3.
 *
 * The mask is taken rather than generated so a test can assert the exact bytes
 * on the wire. It is not a security measure — the payload is already
 * ciphertext by the time it gets here — it exists so that a client cannot be
 * tricked into making a proxy see an HTTP request in the payload.
 */
fun encodeClient(op: WsOp, payload: ByteArray, mask: ByteArray): ByteArray {
    require(mask.size == 4) { "a websocket mask is four bytes" }
    val n = payload.size
    val head = when {
        n < 126 -> 2
        n <= 0xffff -> 4
        else -> 10
    }
    val out = ByteArray(head + 4 + n)
    out[0] = (0x80 or op.code).toByte() // FIN, never fragmented on the way out
    when {
        n < 126 -> out[1] = (0x80 or n).toByte()
        n <= 0xffff -> {
            out[1] = (0x80 or 126).toByte()
            out[2] = (n ushr 8).toByte()
            out[3] = n.toByte()
        }
        else -> {
            out[1] = (0x80 or 127).toByte()
            val long = n.toLong()
            for (i in 0 until 8) out[2 + i] = (long ushr ((7 - i) * 8)).toByte()
        }
    }
    System.arraycopy(mask, 0, out, head, 4)
    for (i in 0 until n) out[head + 4 + i] = (payload[i].toInt() xor mask[i % 4].toInt()).toByte()
    return out
}

/**
 * The sending half of a relay connection.
 *
 * Separate from the reading half, as in `relay.rs`, because the two live on
 * different threads — but *synchronised* here where the Rust side wraps it in
 * an `Arc<Mutex<_>>` at the call site. The read thread answers pings, so two
 * threads really do write, and an interleaved frame is a connection that dies
 * with a protocol error naming nothing.
 */
class WsSender(private val out: OutputStream) {
    /** Send one chunk of the carried byte stream. */
    fun binary(bytes: ByteArray, offset: Int = 0, length: Int = bytes.size) =
        frame(WsOp.BINARY, bytes.copyOfRange(offset, offset + length))

    fun pong(bytes: ByteArray) = frame(WsOp.PONG, bytes)

    fun ping(bytes: ByteArray) = frame(WsOp.PING, bytes)

    /** A clean close: status 1000, "normal closure". */
    fun close() = frame(WsOp.CLOSE, byteArrayOf((CLOSE_NORMAL ushr 8).toByte(), CLOSE_NORMAL.toByte()))

    @Synchronized
    private fun frame(op: WsOp, bytes: ByteArray) {
        val mask = ByteArray(4)
        Relay.random.nextBytes(mask)
        out.write(encodeClient(op, bytes, mask))
        out.flush()
    }

    private companion object {
        /** RFC 6455 §7.4.1: "normal closure", and a status a relay can log. */
        const val CLOSE_NORMAL = 1000
    }
}

/** The reading half of a relay connection. */
class WsReceiver(private val inn: InputStream) {
    /** Bytes of a message whose fragments have not all arrived. */
    private var partial = ByteArray(0)

    /** The opcode the current fragment sequence started with. */
    private var started: WsOp? = null

    /**
     * Read until one complete message is available.
     *
     * Control frames are returned as they arrive — RFC 6455 §5.4 allows one
     * inside a fragmented message, and buffering it into the message would
     * both corrupt the message and stop this end answering keepalives.
     */
    fun message(): WsMessage {
        while (true) {
            val (fin, op, payload) = frame()
            if (op.isControl()) {
                // RFC 6455 §5.5: a control frame is never fragmented and
                // carries at most 125 bytes.
                if (!fin) throw RelayException(RelayError.Protocol("a fragmented control frame"))
                if (payload.size > 125) {
                    throw RelayException(RelayError.Protocol("an oversized control frame"))
                }
                return WsMessage(op, payload)
            }

            val begun = started
            if (op == WsOp.CONTINUATION) {
                if (begun == null) {
                    throw RelayException(
                        RelayError.Protocol("a continuation with nothing to continue"),
                    )
                }
                partial += payload
            } else if (begun != null) {
                throw RelayException(RelayError.Protocol("a new message inside an unfinished one"))
            } else {
                if (fin) return WsMessage(op, payload)
                started = op
                partial = payload
            }
            // Each fragment is legal; the joined message may not be. A cap
            // applied only per frame is no cap at all against a hostile relay.
            if (partial.size > Relay.MAX_FRAME) {
                throw RelayException(RelayError.TooLong(partial.size.toLong()))
            }
            if (fin) {
                val joined = WsMessage(started ?: WsOp.BINARY, partial)
                started = null
                partial = ByteArray(0)
                return joined
            }
        }
    }

    /** One frame off the wire, header and all. */
    private fun frame(): Triple<Boolean, WsOp, ByteArray> {
        val head = readExactly(2)
        val fin = head[0].toInt() and 0x80 != 0
        if (head[0].toInt() and 0x70 != 0) {
            // No extension was negotiated, so a reserved bit set means the far
            // end is speaking a protocol this client did not agree to.
            throw RelayException(RelayError.Protocol("a reserved bit is set"))
        }
        val op = WsOp.fromCode(head[0].toInt() and 0x0f)
            ?: throw RelayException(RelayError.Protocol("an opcode this client does not know"))

        // RFC 6455 §5.1: a server must not mask. A masked frame from a server
        // is refused rather than unmasked, because accepting it would mean this
        // client cannot tell a relay from a client.
        if (head[1].toInt() and 0x80 != 0) {
            throw RelayException(RelayError.Protocol("a masked frame from the server"))
        }
        val short = head[1].toInt() and 0x7f
        val length: Long = when (short) {
            126 -> {
                val n = readExactly(2)
                ((n[0].toLong() and 0xff) shl 8) or (n[1].toLong() and 0xff)
            }
            127 -> {
                val n = readExactly(8)
                var v = 0L
                // Accumulated as a Long and checked before it is an Int: a
                // 64-bit length read into an Int is a four-gigabyte claim that
                // arrives as a small positive number.
                for (b in n) v = (v shl 8) or (b.toLong() and 0xff)
                if (v < 0 || v > Relay.MAX_FRAME) throw RelayException(RelayError.TooLong(v))
                v
            }
            else -> short.toLong()
        }
        if (length > Relay.MAX_FRAME) throw RelayException(RelayError.TooLong(length))
        // Allocated only after the length has been checked.
        return Triple(fin, op, readExactly(length.toInt()))
    }

    private fun readExactly(n: Int): ByteArray {
        val buf = ByteArray(n)
        var read = 0
        while (read < n) {
            val got = inn.read(buf, read, n - read)
            if (got < 0) throw EOFException("the relay connection ended after $read of $n bytes")
            read += got
        }
        return buf
    }
}

/** The client half of the opening handshake. */
class Opening private constructor(val key: String) {
    /** The bytes of the upgrade request. */
    fun request(endpoint: RelayEndpoint, rendezvous: String, role: Rendezvous.Role): ByteArray =
        (
            "GET ${endpoint.requestTarget(rendezvous, role)} HTTP/1.1\r\n" +
                "Host: ${endpoint.authority()}\r\n" +
                "Upgrade: websocket\r\n" +
                "Connection: Upgrade\r\n" +
                "Sec-WebSocket-Key: $key\r\n" +
                "Sec-WebSocket-Version: 13\r\n" +
                "\r\n"
            ).toByteArray(Charsets.US_ASCII)

    /**
     * Read and check the server's answer, consuming exactly the response head
     * and not one byte of what follows it.
     *
     * Byte-at-a-time on purpose: the first frame may arrive in the same packet
     * as the response, and a buffered read would swallow it — a connection
     * that hangs waiting for bytes which have already arrived. This runs once
     * per connection, so the syscalls do not matter.
     */
    fun accept(from: InputStream) {
        val head = ArrayList<Byte>(256)
        val one = ByteArray(1)
        while (true) {
            val got = try {
                from.read(one)
            } catch (e: IOException) {
                throw RelayException(RelayError.Upgrade("${e.message}"))
            }
            if (got <= 0) {
                throw RelayException(
                    RelayError.Upgrade("it closed the connection without answering"),
                )
            }
            head.add(one[0])
            val n = head.size
            if (n >= 4 &&
                head[n - 4] == CR && head[n - 3] == LF && head[n - 2] == CR && head[n - 1] == LF
            ) {
                break
            }
            // A response head that never ends is a way to make this process
            // grow without ever completing a connection.
            if (n > 16 * 1024) {
                throw RelayException(RelayError.Upgrade("its response head never ended"))
            }
        }
        check(head.toByteArray())
    }

    /** The check itself, over a complete response head. */
    fun check(head: ByteArray) {
        val text = String(head, Charsets.ISO_8859_1)
        val lines = text.split("\r\n")
        val status = lines.firstOrNull().orEmpty()
        val code = status.split(Regex("\\s+")).getOrNull(1).orEmpty()
        if (code != "101") {
            // The relay's own status is the useful part of the message: a 404
            // means the path is wrong, a 409 that no desktop is waiting at this
            // rendezvous, a 401 that the deployment wants a credential.
            throw RelayException(
                RelayError.Upgrade("expected HTTP 101, got \"${status.trim()}\""),
            )
        }

        var upgrade: String? = null
        var connection: String? = null
        var accept: String? = null
        for (line in lines.drop(1)) {
            val colon = line.indexOf(':')
            if (colon < 0) continue
            val value = line.substring(colon + 1).trim()
            when (line.substring(0, colon).trim().lowercase()) {
                "upgrade" -> upgrade = value.lowercase()
                "connection" -> connection = value.lowercase()
                "sec-websocket-accept" -> accept = value
            }
        }
        if (upgrade != "websocket") {
            throw RelayException(RelayError.Upgrade("its 101 did not upgrade to websocket"))
        }
        // Comma-separated by RFC 7230, so a contains rather than an equality.
        if (connection?.contains("upgrade") != true) {
            throw RelayException(
                RelayError.Upgrade("its 101 did not name the upgrade in Connection"),
            )
        }
        when (accept) {
            null -> throw RelayException(
                RelayError.Upgrade("its 101 carried no Sec-WebSocket-Accept"),
            )
            Relay.acceptFor(key) -> Unit
            else -> throw RelayException(
                RelayError.Upgrade(
                    "its Sec-WebSocket-Accept does not match the key this client sent",
                ),
            )
        }
    }

    companion object {
        private const val CR: Byte = 13
        private const val LF: Byte = 10

        /**
         * Start a handshake with a caller-supplied nonce, so a test can pin the
         * RFC's own vector. [Opening.fresh] is what production uses.
         */
        fun withNonce(nonce: ByteArray): Opening {
            require(nonce.size == 16) { "RFC 6455 §4.1 nonce is 16 bytes" }
            // The key header is the nonce in standard padded base64 — the
            // same alphabet the accept value uses, and not this protocol's.
            return Opening(Relay.base64Std(nonce))
        }

        /**
         * Start a handshake with a fresh random nonce.
         *
         * RFC 6455 §4.1 requires it to be unpredictable. It is not a secret and
         * it authenticates nothing: its whole job is to make a cached or
         * replayed 101 response detectable.
         */
        fun fresh(): Opening {
            val nonce = ByteArray(16)
            Relay.random.nextBytes(nonce)
            return withNonce(nonce)
        }
    }
}

/**
 * A joined relay connection, presented as the stream pair everything above the
 * transport already takes.
 *
 * [input] and [output] are what [Client.pair] and [Client.openSession] are
 * handed on the LAN leg by a [java.net.Socket]. That they are the same pair
 * here is the whole design: the relay's notices, its pings and its close are
 * consumed inside this class and no caller can branch on them.
 */
class RelayLink(
    /** The bytes coming off the socket, after the upgrade response. */
    from: InputStream,
    /** The bytes going onto it. */
    to: OutputStream,
    /** What to do with the socket when the link is closed. */
    private val onClose: () -> Unit,
) : Closeable {
    private val sender = WsSender(to)
    private val reader = RelayInput(WsReceiver(from), sender) { close() }
    private val writer = RelayOutput(sender) { close() }
    private var closed = false

    /** The carried byte stream, inbound. End-of-stream when the relay says so. */
    val input: InputStream get() = reader

    /** The carried byte stream, outbound. One binary frame per flush. */
    val output: OutputStream get() = writer

    /**
     * Whether the relay ever said the far end was there.
     *
     * For the connector's log and for a test, never for the protocol: nothing
     * above the transport is allowed to ask.
     */
    val sawPaired: Boolean get() = reader.sawPaired

    /**
     * Close once, whichever half is closed.
     *
     * Idempotent and reached from BOTH streams, because that is how the layer
     * above releases a connection: `Session.close` closes its `output` and
     * then its `input`, and on the LAN leg either of those closes the socket
     * underneath. If these two were the default no-op `InputStream.close` —
     * which they were, and every relayed session leaked a TCP connection to
     * the relay — the phone would hold one open per session for the life of
     * the process while the desktop kept a splice alive for a peer that had
     * gone.
     *
     * The buffered bytes go first, then a Close frame so the relay can tell a
     * hang-up from a dropped connection and free the room, then the socket.
     */
    @Synchronized
    override fun close() {
        if (closed) return
        closed = true
        runCatching { writer.flush() }
        runCatching { sender.close() }
        onClose()
    }
}

/**
 * The inbound half: WebSocket messages back into a byte stream.
 *
 * A relay is free to chunk the carried stream however it likes, so this
 * deliberately does not assume one frame is one Noise message. [Transport]
 * reads a four-byte length and then that many bytes, and gets them from
 * however many frames they happen to span.
 */
private class RelayInput(
    private val receiver: WsReceiver,
    private val sender: WsSender,
    private val closer: () -> Unit,
) : InputStream() {
    private var payload = ByteArray(0)
    private var offset = 0
    private var ended = false
    var sawPaired = false
        private set

    override fun read(): Int {
        val one = ByteArray(1)
        return if (read(one, 0, 1) < 0) -1 else one[0].toInt() and 0xff
    }

    override fun read(b: ByteArray, off: Int, len: Int): Int {
        if (len == 0) return 0
        while (offset >= payload.size) {
            if (!pump()) return -1
        }
        val n = minOf(len, payload.size - offset)
        System.arraycopy(payload, offset, b, off, n)
        offset += n
        return n
    }

    override fun available(): Int = payload.size - offset

    /** Closes the whole link: see [RelayLink.close]. */
    override fun close() {
        ended = true
        closer()
    }

    /** True when [payload] now holds bytes; false at end of stream. */
    private fun pump(): Boolean {
        if (ended) return false
        while (true) {
            val message = try {
                receiver.message()
            } catch (e: EOFException) {
                // The same thing a desktop closing a TCP connection is. An
                // exception with the relay's name on it here would be the
                // layer above learning which leg it is on.
                ended = true
                return false
            }
            when (message.op) {
                WsOp.BINARY -> if (message.payload.isNotEmpty()) {
                    payload = message.payload
                    offset = 0
                    return true
                }
                WsOp.TEXT -> {
                    // The relay's own vocabulary, consumed here. An unknown
                    // notice is ignored, so a relay that grows a word does not
                    // become a flag day for every phone.
                    when (Notice.parse(message.payload)) {
                        Notice.PAIRED -> sawPaired = true
                        Notice.PEER_GONE -> {
                            // The far end went away. End of stream, which is
                            // exactly what it is on the LAN leg.
                            ended = true
                            return false
                        }
                        Notice.WAITING, null -> Unit
                    }
                }
                WsOp.PING -> sender.pong(message.payload)
                WsOp.PONG -> Unit
                WsOp.CLOSE -> {
                    ended = true
                    return false
                }
                WsOp.CONTINUATION -> Unit // joined by the receiver; never surfaces
            }
        }
    }
}

/**
 * The outbound half: a byte stream into binary frames, one per flush.
 *
 * [Transport.writeMessage] writes the four-byte length, the ciphertext, and
 * then flushes, so one flush is one Noise message and one frame. Buffering to
 * the flush rather than framing every `write` is what stops a length prefix
 * travelling in a frame of its own — correct either way, since the stream is
 * a stream, but four bytes per frame is a poor use of somebody's mobile data.
 */
private class RelayOutput(
    private val sender: WsSender,
    private val closer: () -> Unit,
) : OutputStream() {
    private var buffer = ByteArray(8 * 1024)
    private var used = 0

    override fun write(b: Int) {
        if (used == Relay.MAX_FRAME) flush()
        ensure(1)
        buffer[used++] = b.toByte()
    }

    override fun write(b: ByteArray, off: Int, len: Int) {
        var written = 0
        while (written < len) {
            // A write larger than the frame cap becomes several frames rather
            // than one oversized one. The carried stream is a stream; where it
            // is cut is nobody's business above this line.
            if (used == Relay.MAX_FRAME) flush()
            val room = minOf(len - written, Relay.MAX_FRAME - used)
            ensure(room)
            System.arraycopy(b, off + written, buffer, used, room)
            used += room
            written += room
        }
    }

    override fun flush() {
        if (used == 0) return
        val n = used
        // Cleared before the write, so a failed send does not leave the bytes
        // to be sent a second time by the close path.
        used = 0
        sender.binary(buffer, 0, n)
    }

    /** Closes the whole link: see [RelayLink.close]. */
    override fun close() {
        closer()
    }

    private fun ensure(more: Int) {
        if (used + more <= buffer.size) return
        var size = buffer.size
        while (size < used + more) size *= 2
        buffer = buffer.copyOf(minOf(size, Relay.MAX_FRAME))
    }
}
