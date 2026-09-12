package com.apexos.remote.core

import org.bouncycastle.crypto.agreement.X25519Agreement
import org.bouncycastle.crypto.digests.Blake2sDigest
import org.bouncycastle.crypto.digests.SHA256Digest
import org.bouncycastle.crypto.modes.ChaCha20Poly1305
import org.bouncycastle.crypto.params.KeyParameter
import org.bouncycastle.crypto.params.ParametersWithIV
import org.bouncycastle.crypto.params.X25519PrivateKeyParameters
import org.bouncycastle.crypto.params.X25519PublicKeyParameters
import java.security.SecureRandom

/**
 * The primitives the Noise handshake is built from.
 *
 * ## Why the lightweight API and never JCA
 *
 * Every call here reaches `org.bouncycastle.crypto.*` directly — plain objects
 * with no provider registration — rather than `Cipher.getInstance` or
 * `MessageDigest.getInstance`. On a JVM the two would agree. On Android they
 * would not: the platform ships a *stripped* BouncyCastle under the `BC`
 * provider name, Conscrypt owns most of the JCA slots the full one used to
 * hold, and which implementation a name resolves to is an Android version's
 * business rather than this code's. `Cipher.getInstance("ChaCha20-Poly1305")`
 * does not exist below API 28 at all, and BLAKE2s has never been in the
 * platform. The lightweight classes behave identically on a JVM, on a device
 * and in CI, which is the only way a test here means anything about a phone.
 */
internal object Crypto {
    const val HASHLEN = 32
    const val DHLEN = 32
    const val TAGLEN = 16
    private const val BLAKE2S_BLOCK = 64

    private val random = SecureRandom()

    fun blake2s(vararg parts: ByteArray): ByteArray {
        val d = Blake2sDigest()
        for (p in parts) d.update(p, 0, p.size)
        val out = ByteArray(HASHLEN)
        d.doFinal(out, 0)
        return out
    }

    fun sha256(vararg parts: ByteArray): ByteArray {
        val d = SHA256Digest()
        for (p in parts) d.update(p, 0, p.size)
        val out = ByteArray(d.digestSize)
        d.doFinal(out, 0)
        return out
    }

    /**
     * HMAC over BLAKE2s, spelled out rather than taken from `HMac`.
     *
     * `org.bouncycastle.crypto.macs.HMac` would work, but it takes the block
     * length from the digest's own `getByteLength()`, and this construction is
     * load-bearing enough — every key in the protocol comes out of it — that
     * the 64-byte block belongs written down where a reader can check it
     * against RFC 2104 and against the Noise specification's HASHLEN/BLOCKLEN
     * table.
     */
    fun hmacBlake2s(key: ByteArray, data: ByteArray): ByteArray {
        var k = key
        if (k.size > BLAKE2S_BLOCK) k = blake2s(k)
        if (k.size < BLAKE2S_BLOCK) k = k.copyOf(BLAKE2S_BLOCK)
        val ipad = ByteArray(BLAKE2S_BLOCK) { (k[it].toInt() xor 0x36).toByte() }
        val opad = ByteArray(BLAKE2S_BLOCK) { (k[it].toInt() xor 0x5c).toByte() }
        return blake2s(opad, blake2s(ipad, data))
    }

    /**
     * Noise's own HKDF: two outputs, and no `info` field.
     *
     * Not RFC 5869. The Noise specification (§4.3) defines HKDF with a fixed
     * chain of HMACs over single counter bytes and no context string, and a
     * caller that reached for a general HKDF would get different keys and a
     * handshake that fails with an error about the library.
     */
    fun hkdf2(chainingKey: ByteArray, ikm: ByteArray): Pair<ByteArray, ByteArray> {
        val tempKey = hmacBlake2s(chainingKey, ikm)
        val out1 = hmacBlake2s(tempKey, byteArrayOf(0x01))
        val out2 = hmacBlake2s(tempKey, out1 + byteArrayOf(0x02))
        return out1 to out2
    }

    /**
     * The 12-byte ChaCha20-Poly1305 nonce Noise specifies: four zero bytes,
     * then the 64-bit counter **little-endian**.
     *
     * Little-endian is the one detail in this file that is easy to get wrong
     * and impossible to notice locally: two implementations that both chose
     * big-endian would interoperate perfectly with each other and with nothing
     * else. The cross-implementation vectors in `src/test/resources` are what
     * catch it.
     */
    fun nonce(counter: Long): ByteArray {
        val out = ByteArray(12)
        for (i in 0 until 8) out[4 + i] = ((counter ushr (i * 8)) and 0xff).toByte()
        return out
    }

    fun encrypt(key: ByteArray, nonce: ByteArray, ad: ByteArray, plaintext: ByteArray): ByteArray {
        val c = ChaCha20Poly1305()
        c.init(true, ParametersWithIV(KeyParameter(key), nonce))
        c.processAADBytes(ad, 0, ad.size)
        val out = ByteArray(c.getOutputSize(plaintext.size))
        var n = c.processBytes(plaintext, 0, plaintext.size, out, 0)
        n += c.doFinal(out, n)
        return if (n == out.size) out else out.copyOf(n)
    }

    /** Throws [org.bouncycastle.crypto.InvalidCipherTextException] when the tag does not verify. */
    fun decrypt(key: ByteArray, nonce: ByteArray, ad: ByteArray, ciphertext: ByteArray): ByteArray {
        val c = ChaCha20Poly1305()
        c.init(false, ParametersWithIV(KeyParameter(key), nonce))
        c.processAADBytes(ad, 0, ad.size)
        val out = ByteArray(c.getOutputSize(ciphertext.size))
        var n = c.processBytes(ciphertext, 0, ciphertext.size, out, 0)
        n += c.doFinal(out, n)
        return if (n == out.size) out else out.copyOf(n)
    }

    fun x25519(secret: ByteArray, remotePublic: ByteArray): ByteArray {
        val a = X25519Agreement()
        a.init(X25519PrivateKeyParameters(secret, 0))
        val out = ByteArray(a.agreementSize)
        a.calculateAgreement(X25519PublicKeyParameters(remotePublic, 0), out, 0)
        return out
    }

    fun publicFromSecret(secret: ByteArray): ByteArray =
        X25519PrivateKeyParameters(secret, 0).generatePublicKey().encoded

    fun generateSecret(): ByteArray {
        val raw = ByteArray(DHLEN)
        random.nextBytes(raw)
        // Clamping is what `X25519PrivateKeyParameters` does on the way in, so
        // the stored bytes and the bytes the agreement uses are the same and a
        // key round-trips through storage unchanged.
        return X25519PrivateKeyParameters(raw, 0).encoded
    }

    /**
     * Compare without leaking where two byte strings differ through timing.
     *
     * Used for the pairing token, which is the one secret in this protocol
     * that is compared rather than proved by a handshake.
     */
    fun constantTimeEquals(a: ByteArray, b: ByteArray): Boolean {
        if (a.size != b.size) return false
        var diff = 0
        for (i in a.indices) diff = diff or (a[i].toInt() xor b[i].toInt())
        return diff == 0
    }
}
