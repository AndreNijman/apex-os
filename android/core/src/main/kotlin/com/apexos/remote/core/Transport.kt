package com.apexos.remote.core

import java.io.EOFException
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream

/**
 * The length-prefixed transport, and the one plaintext byte in front of it.
 *
 * Each Noise message travels as `[u32 big-endian length][ciphertext]`. The
 * length is the transport's, not the frame's: the receiver has to know how
 * many bytes to decrypt before it can see anything inside, and a length
 * written twice is two lengths that can disagree.
 *
 * Ported from `apexd/apex-remoted/src/net.rs`.
 */
object Transport {
    /**
     * The device's first byte: which handshake follows.
     *
     * A byte rather than a negotiation. Both values lead to a Noise handshake
     * that either completes or does not, so an observer flipping it produces a
     * failed connection rather than a downgrade.
     */
    const val HELLO_PAIR: Byte = 'P'.code.toByte()
    const val HELLO_SESSION: Byte = 'S'.code.toByte()

    /** The largest message the transport will read. */
    const val MAX_MESSAGE = Noise.MAX_MESSAGE

    fun writeMessage(out: OutputStream, message: ByteArray) {
        if (message.size > MAX_MESSAGE) {
            throw IOException("a message of ${message.size} bytes exceeds $MAX_MESSAGE")
        }
        val header = ByteArray(4)
        header[0] = (message.size ushr 24).toByte()
        header[1] = (message.size ushr 16).toByte()
        header[2] = (message.size ushr 8).toByte()
        header[3] = message.size.toByte()
        out.write(header)
        out.write(message)
        out.flush()
    }

    fun readMessage(input: InputStream): ByteArray {
        val header = readExactly(input, 4)
        val length = ((header[0].toInt() and 0xff) shl 24) or
            ((header[1].toInt() and 0xff) shl 16) or
            ((header[2].toInt() and 0xff) shl 8) or
            (header[3].toInt() and 0xff)
        // Refused before allocating. A four-byte header claiming four
        // gigabytes is the cheapest denial of service there is, and this end
        // runs on a phone where it is also the cheapest way to be killed by
        // the low-memory killer.
        if (length < 0 || length > MAX_MESSAGE) {
            throw IOException("a peer announced a $length-byte message; the limit is $MAX_MESSAGE")
        }
        return readExactly(input, length)
    }

    private fun readExactly(input: InputStream, n: Int): ByteArray {
        val buf = ByteArray(n)
        var read = 0
        while (read < n) {
            val got = input.read(buf, read, n - read)
            // A connection that dies mid-message must be an error and never a
            // short message: a truncated frame handed to the decoder above
            // would be a keystroke the far end never sent.
            if (got < 0) throw EOFException("the connection ended after $read of $n bytes")
            read += got
        }
        return buf
    }
}
