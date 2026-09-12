package com.apexos.remote.core

/**
 * The base64 alphabet used everywhere in this protocol: URL-safe, unpadded.
 *
 * URL-safe because these strings go in a QR code, in a relay path and on a
 * command line, and `+` and `/` are wrong in at least two of those. Unpadded
 * because `=` is the third character that is wrong in a URL and every length
 * here is fixed anyway.
 *
 * Hand-rolled rather than [java.util.Base64]. The JDK's URL decoder accepts a
 * padded string — `Base64.getUrlDecoder().decode("<43 chars>=")` returns 32
 * bytes without complaint — and the desktop's decoder
 * (`data_encoding::BASE64URL_NOPAD`) refuses it. A device that accepted what
 * the desktop rejects would treat two different strings as the same key, which
 * is the beginning of every "why does it work on my phone" bug. It also
 * refuses non-canonical trailing bits, which the JDK ignores.
 */
object Base64Url {
    private const val ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"

    private val REVERSE = IntArray(128) { -1 }.also { table ->
        ALPHABET.forEachIndexed { index, c -> table[c.code] = index }
    }

    fun encode(bytes: ByteArray): String {
        val out = StringBuilder((bytes.size * 4 + 2) / 3)
        var i = 0
        while (i + 3 <= bytes.size) {
            val n = ((bytes[i].toInt() and 0xff) shl 16) or
                ((bytes[i + 1].toInt() and 0xff) shl 8) or
                (bytes[i + 2].toInt() and 0xff)
            out.append(ALPHABET[(n ushr 18) and 0x3f])
            out.append(ALPHABET[(n ushr 12) and 0x3f])
            out.append(ALPHABET[(n ushr 6) and 0x3f])
            out.append(ALPHABET[n and 0x3f])
            i += 3
        }
        when (bytes.size - i) {
            1 -> {
                val n = (bytes[i].toInt() and 0xff) shl 16
                out.append(ALPHABET[(n ushr 18) and 0x3f])
                out.append(ALPHABET[(n ushr 12) and 0x3f])
            }
            2 -> {
                val n = ((bytes[i].toInt() and 0xff) shl 16) or ((bytes[i + 1].toInt() and 0xff) shl 8)
                out.append(ALPHABET[(n ushr 18) and 0x3f])
                out.append(ALPHABET[(n ushr 12) and 0x3f])
                out.append(ALPHABET[(n ushr 6) and 0x3f])
            }
        }
        return out.toString()
    }

    /**
     * Decode, or `null`. Never a partial decode: a key that is nearly valid is
     * not a key.
     */
    fun decode(text: String): ByteArray? {
        val n = text.length
        // A base64 group is 2, 3 or 4 characters. One leftover character
        // encodes six bits, which is not a byte and is therefore not an
        // encoding of anything.
        if (n % 4 == 1) return null
        val out = ByteArray(n / 4 * 3 + when (n % 4) { 2 -> 1; 3 -> 2; else -> 0 })
        var acc = 0
        var bits = 0
        var written = 0
        for (c in text) {
            val v = if (c.code < 128) REVERSE[c.code] else -1
            // `=` lands here along with `+`, `/`, whitespace and everything
            // else outside the alphabet. Padding is not "tolerated then
            // ignored": it is a different string, and the desktop refuses it.
            if (v < 0) return null
            acc = (acc shl 6) or v
            bits += 6
            if (bits >= 8) {
                bits -= 8
                out[written++] = ((acc ushr bits) and 0xff).toByte()
            }
        }
        // The bits left over after the last whole byte must be zero. They are
        // in a canonical encoding, and a decoder that ignored them would map
        // several distinct strings onto one key.
        if (bits > 0 && (acc and ((1 shl bits) - 1)) != 0) return null
        return out
    }
}
