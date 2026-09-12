package com.apexos.remote.core

import java.util.Base64

/**
 * Looking for material that should not be somewhere, in every shape it could
 * plausibly arrive in.
 *
 * Shared by [SecretStorageTest], which scans the bytes [MachineStore.encode]
 * produces, and by [InsecureStorageTest], which scans every file a real pairing
 * and a real session leave behind in a directory. One scan and not two on
 * purpose: `SecretStorageTest.theSearchAlsoCatchesEveryOtherEncodingItClaimsTo`
 * checks each branch of it against a file that really does contain the needle,
 * and a second copy of the scan would be a second copy nothing checks.
 *
 * The needle is a `ByteArray` rather than a key type because the things being
 * looked for are not all keys: a device secret, a pairing token and a line of
 * somebody's terminal are all just bytes that must not be on disk.
 */
object Traces {
    /**
     * Every encoding of [needle] that appears in [bytes], named.
     *
     * base64url is what this protocol uses; standard base64 is what a careless
     * `java.util.Base64` reaches for; hex in either case is what a debug helper
     * prints; UTF-8 is how a line of terminal output would arrive; and the
     * raw-byte window catches an encoding nobody listed.
     */
    fun of(needle: ByteArray, bytes: ByteArray): List<String> {
        val text = String(bytes, Charsets.UTF_8)
        val found = mutableListOf<String>()
        fun check(label: String, haystackNeedle: String) {
            if (haystackNeedle.isNotEmpty() && text.contains(haystackNeedle)) found += label
        }
        check("base64url", Base64Url.encode(needle))
        if (needle.size >= 16) {
            check("base64url of the first half", Base64Url.encode(needle.copyOf(16)))
        }
        check("standard base64", Base64.getEncoder().encodeToString(needle))
        check("standard base64 unpadded", Base64.getEncoder().withoutPadding().encodeToString(needle))
        check("base64url via the JDK", Base64.getUrlEncoder().withoutPadding().encodeToString(needle))
        check("lower-case hex", Vectors.encodeHex(needle))
        check("upper-case hex", Vectors.encodeHex(needle).uppercase())
        check("a JSON byte array", needle.joinToString(",") { (it.toInt() and 0xff).toString() })
        // The catch-all: the raw bytes anywhere in the file, in any framing.
        // This is also the branch that finds plain UTF-8 text, which is how a
        // leaked line of terminal output would actually look.
        for (i in 0..bytes.size - needle.size) {
            if (bytes.copyOfRange(i, i + needle.size).contentEquals(needle)) {
                found += "the raw bytes at offset $i"
                break
            }
        }
        return found
    }
}
