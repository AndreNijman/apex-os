package com.apexos.remote.core

import org.junit.jupiter.api.Assertions.assertArrayEquals
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import java.security.SecureRandom
import java.util.Base64
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * P1-053: "no secrets, private keys or session plaintext are written to
 * insecure app storage."
 *
 * ## Why this is a search and not an assertion
 *
 * The easy version of this test asserts that the code put the key somewhere
 * safe. That proves nothing: it restates the author's intention in a second
 * place, and it passes just as happily when a second code path writes the key
 * somewhere else. "Permission denied is not absence" has a sibling — *"I put
 * it elsewhere" is not absence* — and the only way past both is to take the
 * bytes that actually land on disk and go looking.
 *
 * So [theWrittenStoreContainsNoTraceOfThePrivateKey] serialises a store
 * holding a real X25519 keypair and searches the resulting bytes for that key
 * in every encoding it could plausibly take, plus a sliding window over the
 * raw bytes for any encoding nobody thought of.
 *
 * ## And why the search is itself checked
 *
 * A search that cannot fail is not evidence either. [theSameSearchFindsAKeyThatIsWrittenInTheClear]
 * runs the identical scan over a deliberately insecure store and requires it to
 * catch every encoding. If someone weakens the scan, that test goes red first.
 *
 * ## What this does NOT prove, stated plainly
 *
 * The [SecretBox] here is AES-GCM with a key generated in this JVM. On a phone
 * it is an AES key in the platform keystore created with
 * `setUserAuthenticationRequired(true)`, which is what makes unwrapping need a
 * biometric and keeps the wrapping key out of the app's address space. That
 * part is Android-runtime behaviour and cannot be exercised on a machine with
 * no device and no emulator; it is not claimed here. What IS proved here is
 * the half that is platform-independent and that a keystore cannot fix: the
 * store's own serialisation never emits key material, whichever box is wired
 * into it.
 */
class SecretStorageTest {
    /**
     * A real AES-GCM box, standing in for the platform keystore.
     *
     * Real and not a no-op on purpose: a box that returned its input would make
     * the scan below fail, which is the correct outcome and is exactly what
     * [theSameSearchFindsAKeyThatIsWrittenInTheClear] demonstrates.
     */
    private class AesGcmBox(private val key: SecretKey) : SecretBox {
        override val describe = "AES-GCM (test double for the platform keystore)"

        override fun seal(plaintext: ByteArray): ByteArray {
            val iv = ByteArray(12).also { SecureRandom().nextBytes(it) }
            val c = Cipher.getInstance("AES/GCM/NoPadding")
            c.init(Cipher.ENCRYPT_MODE, key, GCMParameterSpec(128, iv))
            return iv + c.doFinal(plaintext)
        }

        override fun open(ciphertext: ByteArray): ByteArray {
            val c = Cipher.getInstance("AES/GCM/NoPadding")
            c.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, ciphertext.copyOf(12)))
            return c.doFinal(ciphertext, 12, ciphertext.size - 12)
        }

        companion object {
            fun generate(): AesGcmBox =
                AesGcmBox(KeyGenerator.getInstance("AES").apply { init(256) }.generateKey())
        }
    }

    /**
     * A box that does nothing, which is what "insecure app storage" looks like.
     *
     * It still copies, because [SecretBox.seal] says an implementation must not
     * retain the array it is handed — the caller wipes it. Returning the same
     * reference made this double store thirty-two zero bytes and quietly turned
     * the leak test green, which is the exact shape of bug that makes a
     * negative result worthless.
     */
    private class NoBox : SecretBox {
        override val describe = "nothing at all"
        override fun seal(plaintext: ByteArray) = plaintext.copyOf()
        override fun open(ciphertext: ByteArray) = ciphertext.copyOf()
    }

    private fun offer(desktopKey: String) = PairingOffer(
        v = REMOTE_PROTOCOL_VERSION,
        machine = "l16",
        key = desktopKey,
        token = Base64Url.encode(ByteArray(Pairing.TOKEN_BYTES) { 7 }),
        lan = listOf("192.168.1.10:7717"),
        relay = "https://apex-relay.andrenijman.com",
        expiresMs = 1_757_000_000_000L,
    )

    /**
     * Every encoding a 32-byte key could reach a file in.
     *
     * base64url is what this protocol uses; standard base64 is what a careless
     * `java.util.Base64` reaches for; hex in both cases is what a debug helper
     * prints; and the raw-byte window catches an encoding nobody listed.
     */
    private fun tracesOf(secret: ByteArray, bytes: ByteArray): List<String> = Traces.of(secret, bytes)

    @Test
    fun theWrittenStoreContainsNoTraceOfThePrivateKey() {
        val identity = InMemoryStaticKey.generate()
        val secret = identity.exportSecretForSealing()
        val box = AesGcmBox.generate()
        val machine = MachineStore.record(
            identity = identity,
            offer = offer(Vectors.text("desktop_key_b64")),
            answer = PairingAnswer(ok = true, device = identity.deviceId(), machine = "l16"),
            box = box,
            nowMs = 1_757_000_000_000L,
        )
        val written = MachineStore().with(machine).encode().toByteArray(Charsets.UTF_8)

        val traces = tracesOf(secret, written)
        assertTrue(
            traces.isEmpty(),
            "the device's private key reached app storage as: ${traces.joinToString(", ")}\n" +
                String(written, Charsets.UTF_8),
        )

        // And the things that SHOULD be there are, so this is not passing
        // because the store wrote nothing at all.
        val text = String(written, Charsets.UTF_8)
        assertTrue(text.contains(identity.publicKeyText()), "the public key is missing from the store")
        assertTrue(text.contains(Vectors.text("desktop_key_b64")), "the pinned desktop key is missing")
        assertTrue(text.contains("l16"))
    }

    @Test
    fun theSameSearchFindsAKeyThatIsWrittenInTheClear() {
        // The mutation, built into the suite: an insecure box, the identical
        // scan, and a demand that it catch the key. A scan that cannot fail
        // would let the test above pass for the wrong reason, so this is what
        // makes that one evidence.
        val identity = InMemoryStaticKey.generate()
        val secret = identity.exportSecretForSealing()
        val machine = MachineStore.record(
            identity = identity,
            offer = offer(Vectors.text("desktop_key_b64")),
            answer = PairingAnswer(ok = true, device = identity.deviceId(), machine = "l16"),
            box = NoBox(),
            nowMs = 0L,
        )
        val written = MachineStore().with(machine).encode().toByteArray(Charsets.UTF_8)
        val traces = tracesOf(secret, written)
        assertTrue(traces.contains("base64url"), "the scan missed a key stored as plain base64url")
        assertTrue(traces.size >= 2, "the scan found only $traces")
    }

    @Test
    fun theSearchAlsoCatchesEveryOtherEncodingItClaimsTo() {
        // Each branch of the scan, exercised against a file that really does
        // contain the key in that form. Without this, a typo in one `check`
        // would silently remove a whole class of leak from the search.
        val secret = ByteArray(32) { (it * 7 + 3).toByte() }
        val forms = mapOf(
            "base64url" to Base64Url.encode(secret),
            "standard base64" to Base64.getEncoder().encodeToString(secret),
            "lower-case hex" to Vectors.encodeHex(secret),
            "upper-case hex" to Vectors.encodeHex(secret).uppercase(),
            "a JSON byte array" to secret.joinToString(",") { (it.toInt() and 0xff).toString() },
        )
        for ((label, form) in forms) {
            val file = """{"v":1,"leak":"$form"}""".toByteArray(Charsets.UTF_8)
            assertTrue(label in tracesOf(secret, file), "the scan missed the key encoded as $label")
        }
        // And the raw-byte window, which is the branch that catches an encoding
        // nobody listed.
        val raw = "prefix".toByteArray() + secret + "suffix".toByteArray()
        assertTrue(tracesOf(secret, raw).any { it.startsWith("the raw bytes") })
        // A file with no key in it finds nothing, so the scan is not simply
        // always positive.
        assertEquals(emptyList<String>(), tracesOf(secret, """{"v":1,"machines":[]}""".toByteArray()))
    }

    @Test
    fun aStoredMachineCanBeOpenedBackIntoAWorkingIdentity() {
        // Sealing it is only useful if it comes back. The unsealed key must
        // produce the same public half and the same device id, or the desktop
        // will not recognise this device after a restart.
        val identity = InMemoryStaticKey.generate()
        val box = AesGcmBox.generate()
        val machine = MachineStore.record(
            identity = identity,
            offer = offer(Vectors.text("desktop_key_b64")),
            answer = PairingAnswer(ok = true, device = identity.deviceId(), machine = "l16"),
            box = box,
            nowMs = 1L,
        )
        val store = MachineStore.decode(MachineStore().with(machine).encode())
        val reloaded = store.find(identity.deviceId())!!
        val key = store.identityFor(reloaded, box)
        assertArrayEquals(identity.publicKey, key.publicKey)
        assertEquals(identity.deviceId(), key.deviceId())
        // And it still does the one thing a static key is for.
        val peer = InMemoryStaticKey.generate()
        assertArrayEquals(key.agree(peer.publicKey), peer.agree(identity.publicKey))
    }

    @Test
    fun aStoreHoldsSeveralMachinesEachWithItsOwnDeviceKey() {
        // P1-053 asks for multiple APEX computers. A device key per machine and
        // not one shared key: revoking this phone on a work laptop must not
        // hand anyone the key it uses with the home one.
        val box = AesGcmBox.generate()
        val work = InMemoryStaticKey.generate()
        val home = InMemoryStaticKey.generate()
        assertFalse(work.publicKey.contentEquals(home.publicKey))
        var store = MachineStore()
        for ((identity, name) in listOf(work to "katana", home to "l16")) {
            store = store.with(
                MachineStore.record(
                    identity = identity,
                    offer = offer(Vectors.text("desktop_key_b64")),
                    answer = PairingAnswer(ok = true, device = identity.deviceId(), machine = name),
                    box = box,
                    nowMs = 1L,
                ),
            )
        }
        assertEquals(2, store.machines.size)
        assertEquals("katana", store.find(work.deviceId())!!.machine)
        assertEquals("l16", store.find(home.deviceId())!!.machine)
        // Neither key appears anywhere in the written store.
        val written = store.encode().toByteArray(Charsets.UTF_8)
        assertEquals(emptyList<String>(), tracesOf(work.exportSecretForSealing(), written))
        assertEquals(emptyList<String>(), tracesOf(home.exportSecretForSealing(), written))
        // Revoking one leaves the other.
        store = store.without(work.deviceId())
        assertEquals(1, store.machines.size)
        assertEquals("l16", store.machines.single().machine)
    }

    @Test
    fun nothingHoldingAKeyPrintsItWhenFormatted() {
        // Android formats objects into logcat and into crash reports, and a
        // default `toString` on a class holding a private key is how a key
        // reaches both. Checked on every type that can hold one.
        val identity = InMemoryStaticKey.generate()
        val secret = Base64Url.encode(identity.exportSecretForSealing())
        assertFalse(identity.toString().contains(secret), identity.toString())
        assertTrue(identity.toString().contains("redacted"), identity.toString())

        val keyPair = KeyPair(identity.exportSecretForSealing(), identity.publicKey)
        assertFalse(keyPair.toString().contains(secret), keyPair.toString())

        val machine = MachineStore.record(
            identity = identity,
            offer = offer(Vectors.text("desktop_key_b64")),
            answer = PairingAnswer(ok = true, device = identity.deviceId(), machine = "l16"),
            box = AesGcmBox.generate(),
            nowMs = 1L,
        )
        // Not the sealed blob either: it is still the half an attacker needs.
        assertFalse(machine.toString().contains(machine.sealed), machine.toString())
        assertFalse(machine.toString().contains(secret))
    }

    @Test
    fun aPairingRequestOnTheWireCarriesNoKeyMaterial() {
        // The other place a private key could escape: the bytes that leave the
        // phone. Searched with the same scan rather than asserted about.
        val identity = InMemoryStaticKey.generate()
        val request = PairingRequest(
            key = identity.publicKeyText(),
            name = "pixel-8",
            token = Base64Url.encode(ByteArray(Pairing.TOKEN_BYTES) { 7 }),
            userVerification = true,
        )
        val onTheWire = Pairing.json.encodeToString(PairingRequest.serializer(), request)
            .toByteArray(Charsets.UTF_8)
        assertEquals(emptyList<String>(), tracesOf(identity.exportSecretForSealing(), onTheWire))
    }
}
