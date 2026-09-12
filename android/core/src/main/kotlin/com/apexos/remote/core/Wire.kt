package com.apexos.remote.core

/**
 * The frames that travel between this device and a paired APEX machine.
 *
 * A port of `apexd/apex-remote-core/src/wire.rs`, which is the specification.
 * Every constant and every refusal here exists because that file has one, and
 * the tests beside this are that file's tests in Kotlin.
 *
 * The shape:
 * ```text
 * [u8 tag][u32 channel, big-endian][payload ...]
 * ```
 * The frame's own length is not in here: it belongs to the transport, which
 * has to know it in order to decrypt, and writing it twice is how the two
 * disagree.
 */
sealed class Frame {
    /**
     * The channel this frame belongs to.
     *
     * Abstract rather than computed in the base, so that a new frame variant
     * cannot be added without saying which channel it lives on.
     */
    abstract val channel: UInt

    /** One line of `apex-agentd`'s control protocol, without its newline. */
    class Control(val line: ByteArray) : Frame() {
        override val channel: UInt get() = CONTROL_CHANNEL
    }

    /** Open a PTY channel onto a session; the payload is agentd's `attach`. */
    class Open(override val channel: UInt, val request: ByteArray) : Frame()

    /** Terminal bytes, in either direction. */
    class Data(override val channel: UInt, val bytes: ByteArray) : Frame()

    /** A channel is finished. [reason] is empty for an ordinary close. */
    class Close(override val channel: UInt, val reason: String) : Frame()

    /** Keepalive, and the only measurement of connection quality either end has. */
    class Ping(val token: Long) : Frame() {
        override val channel: UInt get() = CONTROL_CHANNEL
    }

    class Pong(val token: Long) : Frame() {
        override val channel: UInt get() = CONTROL_CHANNEL
    }

    /** This frame's tag byte. */
    val tag: Int
        get() = when (this) {
            is Control -> TAG_CONTROL
            is Open -> TAG_OPEN
            is Data -> TAG_DATA
            is Close -> TAG_CLOSE
            is Ping -> TAG_PING
            is Pong -> TAG_PONG
        }

    /**
     * Encode, or say why it cannot be.
     *
     * Throws rather than truncating or splitting. A `Data` frame too long for
     * one message is the writer's problem — it knows the stream is a stream
     * and can split it anywhere, with [dataFrames] — and silently splitting a
     * `Control` frame would deliver half a JSON object.
     */
    fun encode(): ByteArray {
        val payload: ByteArray = when (this) {
            is Control -> {
                if (line.any { it == NEWLINE }) throw WireException(WireError.Malformed(TWO_REQUESTS))
                line
            }
            is Open -> request
            is Data -> bytes
            is Close -> reason.toByteArray(Charsets.UTF_8)
            is Ping -> beLong(token)
            is Pong -> beLong(token)
        }
        if (payload.size > MAX_PAYLOAD) throw WireException(WireError.TooLong(payload.size))
        val out = ByteArray(HEADER + payload.size)
        out[0] = tag.toByte()
        val c = channel.toInt()
        out[1] = (c ushr 24).toByte()
        out[2] = (c ushr 16).toByte()
        out[3] = (c ushr 8).toByte()
        out[4] = c.toByte()
        payload.copyInto(out, HEADER)
        return out
    }

    override fun equals(other: Any?): Boolean {
        if (this === other) return true
        return when {
            this is Control && other is Control -> line.contentEquals(other.line)
            this is Open && other is Open ->
                channel == other.channel && request.contentEquals(other.request)
            this is Data && other is Data ->
                channel == other.channel && bytes.contentEquals(other.bytes)
            this is Close && other is Close -> channel == other.channel && reason == other.reason
            this is Ping && other is Ping -> token == other.token
            this is Pong && other is Pong -> token == other.token
            else -> false
        }
    }

    override fun hashCode(): Int = when (this) {
        is Control -> 31 * tag + line.contentHashCode()
        is Open -> 31 * (31 * tag + channel.hashCode()) + request.contentHashCode()
        is Data -> 31 * (31 * tag + channel.hashCode()) + bytes.contentHashCode()
        is Close -> 31 * (31 * tag + channel.hashCode()) + reason.hashCode()
        is Ping -> 31 * tag + token.hashCode()
        is Pong -> 31 * tag + token.hashCode()
    }

    override fun toString(): String = when (this) {
        // Never the payload. A `Data` frame is somebody's terminal and a
        // `Control` frame is their request; neither belongs in a log line that
        // a crash reporter might upload.
        is Control -> "Control(${line.size} bytes)"
        is Open -> "Open(channel=$channel, ${request.size} bytes)"
        is Data -> "Data(channel=$channel, ${bytes.size} bytes)"
        is Close -> "Close(channel=$channel, reason=${reason.take(64)})"
        is Ping -> "Ping($token)"
        is Pong -> "Pong($token)"
    }

    companion object {
        /**
         * The largest payload one frame may carry.
         *
         * Set by the transport rather than by taste: a Noise transport message
         * is at most 65535 bytes including the 16-byte authentication tag, and
         * this leaves room for the 5-byte header as well.
         */
        const val MAX_PAYLOAD: Int = 65535 - 16 - 5

        /** Channel 0, which every [Control] frame uses. */
        val CONTROL_CHANNEL: UInt = 0u

        const val TAG_CONTROL: Int = 0
        const val TAG_OPEN: Int = 1
        const val TAG_DATA: Int = 2
        const val TAG_CLOSE: Int = 3
        const val TAG_PING: Int = 4
        const val TAG_PONG: Int = 5
        const val HEADER: Int = 5

        private const val NEWLINE: Byte = '\n'.code.toByte()
        internal const val TWO_REQUESTS =
            "a control payload carries one request and must not contain a newline"
        private const val PING_WIDTH = "a ping carries an eight-byte token"

        /**
         * Decode one frame from a complete plaintext message.
         *
         * The transport hands over exactly one message, so there is no partial
         * read to handle here and no length prefix to trust: the caller
         * already knows how many bytes there are because it had to in order to
         * decrypt them.
         */
        fun decode(buf: ByteArray): Frame {
            if (buf.size < HEADER) throw WireException(WireError.Short(buf.size))
            val tag = buf[0].toInt() and 0xff
            val channel = (
                ((buf[1].toInt() and 0xff) shl 24) or
                    ((buf[2].toInt() and 0xff) shl 16) or
                    ((buf[3].toInt() and 0xff) shl 8) or
                    (buf[4].toInt() and 0xff)
                ).toUInt()
            val payload = buf.copyOfRange(HEADER, buf.size)
            if (payload.size > MAX_PAYLOAD) throw WireException(WireError.TooLong(payload.size))
            // Channel discipline, checked on the way in. A `Data` frame on the
            // control channel would be terminal bytes delivered to the request
            // parser, and a `Control` frame on a PTY channel would be a request
            // nothing answers.
            val onControl = channel == CONTROL_CHANNEL
            if ((tag == TAG_CONTROL || tag == TAG_PING || tag == TAG_PONG) && !onControl) {
                throw WireException(WireError.WrongChannel(tag, channel))
            }
            if ((tag == TAG_OPEN || tag == TAG_DATA || tag == TAG_CLOSE) && onControl) {
                throw WireException(WireError.WrongChannel(tag, channel))
            }
            return when (tag) {
                TAG_CONTROL -> {
                    if (payload.any { it == NEWLINE }) {
                        throw WireException(WireError.Malformed(TWO_REQUESTS))
                    }
                    Control(payload)
                }
                TAG_OPEN -> Open(channel, payload)
                TAG_DATA -> Data(channel, payload)
                // Lossy, as `String::from_utf8_lossy` is on the far side: a
                // close reason is shown to a human and a malformed one must not
                // be the thing that ends the connection twice.
                TAG_CLOSE -> Close(channel, String(payload, Charsets.UTF_8))
                TAG_PING, TAG_PONG -> {
                    if (payload.size != 8) throw WireException(WireError.Malformed(PING_WIDTH))
                    val token = readBeLong(payload)
                    if (tag == TAG_PING) Ping(token) else Pong(token)
                }
                // Not skippable. A reader that ignored unknown frames would
                // silently drop half of a protocol it was told it could speak.
                else -> throw WireException(WireError.UnknownTag(tag))
            }
        }

        /**
         * Split a byte run into as many [Data] frames as it needs.
         *
         * The one place the size limit is allowed to matter. Callers push
         * whatever a PTY read produced and get frames that will encode.
         */
        fun dataFrames(channel: UInt, bytes: ByteArray): List<Frame> {
            if (bytes.isEmpty()) return emptyList()
            val out = ArrayList<Frame>((bytes.size + MAX_PAYLOAD - 1) / MAX_PAYLOAD)
            var i = 0
            while (i < bytes.size) {
                val end = minOf(i + MAX_PAYLOAD, bytes.size)
                out.add(Data(channel, bytes.copyOfRange(i, end)))
                i = end
            }
            return out
        }

        private fun beLong(v: Long): ByteArray =
            ByteArray(8) { ((v ushr (56 - it * 8)) and 0xff).toByte() }

        private fun readBeLong(b: ByteArray): Long {
            var v = 0L
            for (x in b) v = (v shl 8) or (x.toLong() and 0xff)
            return v
        }
    }
}

/** Why a frame could not be decoded. */
sealed class WireError {
    /** Fewer bytes than a header. */
    data class Short(val got: Int) : WireError()

    /** A tag this build does not know. */
    data class UnknownTag(val tag: Int) : WireError()

    /**
     * A control frame on a channel other than the control one, or a
     * data/open/close frame on it.
     */
    data class WrongChannel(val tag: Int, val channel: UInt) : WireError()

    /** A payload longer than [Frame.MAX_PAYLOAD]. */
    data class TooLong(val length: Int) : WireError()

    /** A frame whose payload is not the shape its tag requires. */
    data class Malformed(val why: String) : WireError()

    val message: String
        get() = when (this) {
            is Short -> "a frame needs at least ${Frame.HEADER} bytes, got $got"
            is UnknownTag ->
                "frame tag $tag is not one this build speaks; the version handshake should " +
                    "have caught that before anything was sent"
            is WrongChannel -> "frame tag $tag arrived on channel $channel, which is not where it belongs"
            is TooLong -> "a payload of $length bytes exceeds the ${Frame.MAX_PAYLOAD}-byte limit"
            is Malformed -> why
        }
}

class WireException(val error: WireError) : Exception(error.message)
