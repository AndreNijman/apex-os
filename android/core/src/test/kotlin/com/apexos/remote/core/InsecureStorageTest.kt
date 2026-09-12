package com.apexos.remote.core

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.Timeout
import org.junit.jupiter.api.io.TempDir
import java.io.File
import java.io.PipedInputStream
import java.io.PipedOutputStream
import java.security.SecureRandom
import java.util.concurrent.TimeUnit
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * P1-053, last criterion: "no secrets, private keys or session plaintext are
 * written to insecure app storage."
 *
 * ## Why this test exists next to `SecretStorageTest`
 *
 * That one searches the string [MachineStore.encode] returns. This one searches
 * **the directory**. The difference is the whole point: a store that serialises
 * cleanly still leaves a key on disk if something writes a second file, if a
 * crash leaves a `.new` behind, or if a debug helper drops a transcript beside
 * it. So this test drives a real pairing and a real session against a real
 * directory, and then walks every file in it byte by byte.
 *
 * That is only possible because [AppStorage] is the single writer to app
 * storage and takes a plain [File] directory. `:app`'s repository is a two-line
 * adapter over it, and `tools/no-second-write-path.sh` is what stops a second
 * writer appearing later — a scan is only evidence about the bytes it can see.
 *
 * ## Hostile, in the sense the card asks for
 *
 * The failing shape of this test is not "the author forgot to encrypt". It is
 * "the author wrote a test that cannot fail". So [theSameWalkFindsEverythingWhenTheAppIsMadeToLeakIt]
 * repeats the identical walk over a directory somebody deliberately leaked into
 * — the raw device key in one file, a terminal transcript in another, the
 * pairing token in a third — and requires all three to be found. If the walk is
 * ever weakened, that test goes red before this one does.
 *
 * ## What is NOT claimed here
 *
 * Nothing about the Android keystore. The [SecretBox] below is AES-GCM with a
 * key generated in this JVM; on a phone it is a keystore key created with
 * `setUserAuthenticationRequired(true)`, and that is runtime behaviour no
 * machine without a device can exercise. What is proved is the half a keystore
 * cannot fix: whatever box is wired in, the bytes that reach the filesystem
 * contain no device secret, no pairing token and no session plaintext.
 *
 * ## The deadline is not boilerplate
 *
 * Both halves of every handshake here run in this process, one of them on a
 * thread, over pipes. A handshake that cannot complete — the wrong key, a
 * changed prologue — leaves both sides blocked on a read that will never
 * arrive, and without a deadline the suite does not fail: it *hangs*. That was
 * measured rather than imagined. Mutating the session below to use a key the
 * store had not pinned turned a ten-second run into a ten-minute one that
 * finally reported "`:core:test` FAILED" and named no test at all. A failure
 * nobody can read is barely better than no failure.
 */
@Timeout(value = 60, unit = TimeUnit.SECONDS)
class InsecureStorageTest {

    /** Distinctive enough that finding it anywhere is unambiguous. */
    private val typedByTheUser = "SESSION-PLAINTEXT-TYPED-BY-THE-USER-b4a1f0c7".toByteArray()
    private val printedByTheMachine = "SESSION-PLAINTEXT-FROM-THE-MACHINE-9de23a51".toByteArray()

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
            fun generate() = AesGcmBox(KeyGenerator.getInstance("AES").apply { init(256) }.generateKey())
        }
    }

    /** What a pairing produced: everything the scan is later going to look for. */
    private class Paired(
        val machine: PairedMachine,
        val deviceSecret: ByteArray,
        val token: ByteArray,
        /** The desktop half, kept so the session runs against the key that was pinned. */
        val desktop: InMemoryStaticKey,
    )

    /**
     * A whole pairing, both halves, over real streams.
     *
     * Piped streams and a thread rather than a recorded transcript: the desktop
     * half has to *answer*, and a recording could only replay bytes that were
     * made for one fixed device key. Here the device key is fresh on every run,
     * which is what makes the search below a search rather than a lookup.
     */
    private fun pairForReal(box: SecretBox, nowMs: Long): Paired {
        val desktop = InMemoryStaticKey.generate()
        val tokenBytes = ByteArray(Pairing.TOKEN_BYTES).also { SecureRandom().nextBytes(it) }
        val offer = PairingOffer(
            v = REMOTE_PROTOCOL_VERSION,
            machine = "l16",
            key = desktop.publicKeyText(),
            token = Base64Url.encode(tokenBytes),
            lan = listOf("192.168.1.10:7717"),
            relay = null,
            expiresMs = nowMs + Pairing.OFFER_TTL_MS,
        )
        val identity = InMemoryStaticKey.generate()
        val secret = identity.exportSecretForSealing()

        val toDesktop = PipedOutputStream()
        val desktopReads = PipedInputStream(toDesktop, 1 shl 16)
        val toDevice = PipedOutputStream()
        val deviceReads = PipedInputStream(toDevice, 1 shl 16)

        val responder = Thread {
            // The desktop's side of `Noise_NK`: read the hello byte, read
            // message 1, answer inside the finished handshake.
            assertEquals(Transport.HELLO_PAIR.toInt(), desktopReads.read())
            val handshake = Noise.pairingResponder(desktop, REMOTE_PROTOCOL_VERSION)
            handshake.read(Transport.readMessage(desktopReads))
            val answer = PairingAnswer(ok = true, device = identity.deviceId(), machine = "l16")
            Transport.writeMessage(
                toDevice,
                handshake.write(
                    Pairing.json.encodeToString(PairingAnswer.serializer(), answer)
                        .toByteArray(Charsets.UTF_8),
                ),
            )
            toDevice.flush()
        }
        responder.start()
        val answer = Client.pair(
            input = deviceReads,
            output = toDesktop,
            offer = offer,
            identity = identity,
            deviceName = "pixel-8",
            userVerification = true,
        )
        joinOrFail(responder, "the desktop half of the pairing never finished")
        return Paired(
            machine = MachineStore.record(identity, offer, answer, box, nowMs),
            deviceSecret = secret,
            token = tokenBytes,
            desktop = desktop,
        )
    }

    /**
     * A session, with plaintext crossing it in both directions.
     *
     * The desktop half runs `Noise_IK` as responder and then echoes a line of
     * its own, so the test really does hold two distinct strings that were
     * session plaintext on this machine at the moment the directory is walked.
     */
    private fun talkOverASession(paired: Paired, identity: StaticKey) {
        // The key the store pinned at pairing, not a fresh one. That makes the
        // session go through `Device.checkKey(machine.desktopKey)` — the path
        // the app really uses — instead of past it.
        val desktopSecret = paired.desktop
        val machine = paired.machine
        val toDesktop = PipedOutputStream()
        val desktopReads = PipedInputStream(toDesktop, 1 shl 16)
        val toDevice = PipedOutputStream()
        val deviceReads = PipedInputStream(toDevice, 1 shl 16)

        val desktopPublic = Device.checkKey(machine.desktopKey)
        val responder = Thread {
            assertEquals(Transport.HELLO_SESSION.toInt(), desktopReads.read())
            val handshake = Noise.sessionResponder(desktopSecret, REMOTE_PROTOCOL_VERSION)
            handshake.read(Transport.readMessage(desktopReads))
            Transport.writeMessage(toDevice, handshake.write(machine.machine.toByteArray()))
            toDevice.flush()
            val channel = handshake.intoTransport()
            // Read what the phone typed, then answer with output of its own.
            val typed = Frame.decode(channel.open(Transport.readMessage(desktopReads)))
            check(typed is Frame.Data)
            Transport.writeMessage(
                toDevice,
                channel.seal(Frame.Data(1u, printedByTheMachine).encode()),
            )
            toDevice.flush()
        }
        responder.start()
        val session = Client.openSession(
            input = deviceReads,
            output = toDesktop,
            identity = identity,
            desktopPublic = desktopPublic,
        )
        session.send(Frame.Data(1u, typedByTheUser))
        val back = session.receive()
        check(back is Frame.Data && back.bytes.contentEquals(printedByTheMachine))
        joinOrFail(responder, "the desktop half of the session never finished")
    }

    /**
     * Wait for the far half, and say so when it does not come back.
     *
     * A responder blocked on a read it will never satisfy is the shape every
     * failure in this file takes, and `Thread.join()` with no argument turns
     * that into a suite that never returns.
     */
    private fun joinOrFail(thread: Thread, what: String) {
        thread.join(20_000)
        if (thread.isAlive) {
            thread.interrupt()
            throw AssertionError("$what — it is still blocked after twenty seconds")
        }
    }

    /** Every file under [directory], with its bytes. */
    private fun everyFileIn(directory: File): List<Pair<File, ByteArray>> =
        directory.walkTopDown().filter { it.isFile }.map { it to it.readBytes() }.toList()

    @Test
    fun `a real pairing and a real session leave nothing on disk that they should not`() {
        val directory = createTempDirectory()
        val storage = AppStorage(directory)
        val box = AesGcmBox.generate()

        val paired = pairForReal(box, nowMs = 1_757_000_000_000L)
        storage.save(storage.load().with(paired.machine))

        // Reload the way the app would after a restart, unseal, and use the
        // identity for a session. Everything the app does with a key, done.
        val reloaded = storage.load()
        val identity = reloaded.identityFor(reloaded.find(paired.machine.deviceId)!!, box)
        talkOverASession(paired, identity)

        // A second write, because a preference change is a write and the
        // criterion is about what is on disk at any moment, not only after the
        // first one.
        storage.save(reloaded.copy(settings = Settings(dynamicColour = true)))

        val files = everyFileIn(directory)
        // Non-vacuous: the walk found something, and that something is the
        // store. A scan over an empty directory would pass for free.
        assertTrue(files.isNotEmpty(), "the walk found no files at all, so it proved nothing")
        val storeText = String(storage.storeFile.readBytes(), Charsets.UTF_8)
        assertTrue(storeText.contains(paired.machine.deviceKey), "the store is missing the public key")
        assertTrue(storeText.contains("l16"), "the store is missing the machine name")

        val leaks = mutableListOf<String>()
        for ((file, bytes) in files) {
            for ((what, needle) in needles(paired)) {
                for (form in Traces.of(needle, bytes)) {
                    leaks += "${file.relativeTo(directory)}: $what as $form"
                }
            }
        }
        assertEquals(
            emptyList<String>(),
            leaks,
            "material that must never reach app storage was found there:\n" + leaks.joinToString("\n"),
        )

        // And the sealed blob really is in there, so the store is not clean
        // because it stored nothing.
        assertTrue(storeText.contains(paired.machine.sealed), "the sealed key is missing from the store")
    }

    @Test
    fun `the same walk finds everything when the app is made to leak it`() {
        // The mutation, built in. An app that wrote its key, its user's
        // keystrokes and its pairing token to app storage — each in a different
        // shape, each in a different file, one of them in a leftover `.new`
        // that a crash would plausibly produce. Every one must be found.
        val directory = createTempDirectory()
        val storage = AppStorage(directory)
        val box = AesGcmBox.generate()
        val paired = pairForReal(box, nowMs = 1L)
        storage.save(storage.load().with(paired.machine))

        File(directory, "debug.log").writeText(
            "device key ${Base64Url.encode(paired.deviceSecret)}\n",
        )
        File(directory, "scrollback.txt").writeBytes(
            "$ ".toByteArray() + typedByTheUser + "\n".toByteArray() + printedByTheMachine,
        )
        File(directory, "${AppStorage.STORE_NAME}.new").writeText(
            """{"token":"${Vectors.encodeHex(paired.token)}"}""",
        )

        val found = mutableListOf<String>()
        for ((file, bytes) in everyFileIn(directory)) {
            for ((what, needle) in needles(paired)) {
                if (Traces.of(needle, bytes).isNotEmpty()) found += "$what in ${file.name}"
            }
        }
        assertTrue("the device's private key in debug.log" in found, "missed the key: $found")
        assertTrue("what the user typed in scrollback.txt" in found, "missed the keystrokes: $found")
        assertTrue("what the machine printed in scrollback.txt" in found, "missed the output: $found")
        assertTrue(
            "the pairing token in ${AppStorage.STORE_NAME}.new" in found,
            "missed the token in the leftover scratch file: $found",
        )
        assertEquals(4, found.size, "the walk found $found")
    }

    @Test
    fun `the scratch file a save writes beside the store does not survive it`() {
        // The leak above is not hypothetical: `save` writes beside and renames,
        // so there is a moment when a second file holds the same sealed key.
        // What must not happen is that it is still there afterwards — a `.new`
        // nobody cleans up is one more copy for a scan, and a backup, to find.
        val directory = createTempDirectory()
        val storage = AppStorage(directory)
        val paired = pairForReal(AesGcmBox.generate(), nowMs = 1L)
        storage.save(storage.load().with(paired.machine))
        assertEquals(
            listOf(AppStorage.STORE_NAME),
            everyFileIn(directory).map { it.first.name }.sorted(),
            "a save left something behind besides the store",
        )
    }

    @Test
    fun `a save that fails leaves no scratch file holding the sealed key`() {
        // The path the test above does NOT cover, which is worth saying out
        // loud: when the rename succeeds there is nothing left to clean up, so
        // that test is really about the rename. This one is about the `finally`.
        //
        // The failure is forced rather than mocked. `machines.json` is made a
        // directory, which is a thing an unpacked backup or a stray `mkdir`
        // really can produce: the rename onto it fails, the fallback write onto
        // it fails too, and `save` throws. What must not be true afterwards is
        // that a full copy of the store — sealed key and all — is sitting in
        // `machines.json.new` waiting for the next thing that reads the folder.
        val directory = createTempDirectory()
        val storage = AppStorage(directory)
        val paired = pairForReal(AesGcmBox.generate(), nowMs = 1L)
        File(directory, AppStorage.STORE_NAME).mkdirs()
        File(directory, "${AppStorage.STORE_NAME}/occupied").writeText("in the way")

        val store = MachineStore().with(paired.machine)
        var threw = false
        try {
            storage.save(store)
        } catch (_: Exception) {
            threw = true
        }
        assertTrue(threw, "saving onto a directory somehow succeeded, so this test proves nothing")
        val strays = everyFileIn(directory).map { it.first.relativeTo(directory).path }
        assertEquals(
            listOf("${AppStorage.STORE_NAME}/occupied"),
            strays.sorted(),
            "a failed save left a scratch file behind: $strays",
        )
    }

    @Test
    fun `a store that will not parse is not a store that throws`() {
        // The recovery path, because an app that crashed on a damaged file
        // would be an app whose owner reaches for a backup — and a backup is
        // the one place the sealed key is worth nothing and dangerous anyway.
        val directory = createTempDirectory()
        val storage = AppStorage(directory)
        storage.storeFile.writeText("{ this is not json")
        assertEquals(MachineStore(), storage.load())
    }

    private fun needles(paired: Paired) = listOf(
        "the device's private key" to paired.deviceSecret,
        "what the user typed" to typedByTheUser,
        "what the machine printed" to printedByTheMachine,
        "the pairing token" to paired.token,
    )

    @TempDir
    lateinit var tempRoot: File

    private var directories = 0

    /** A fresh subdirectory per use, so one test's files cannot answer another's walk. */
    private fun createTempDirectory(): File =
        File(tempRoot, "files-${directories++}").also { it.mkdirs() }
}
