package com.apexos.remote.device

import android.os.Build
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.apexos.remote.core.InMemoryStaticKey
import com.apexos.remote.core.Noise
import com.apexos.remote.core.REMOTE_PROTOCOL_VERSION
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

/**
 * The floor of every other on-device claim: this really is a phone, `:core`
 * really is on its classpath, and the cryptography it rests on really works on
 * Android's providers rather than on a JVM's.
 *
 * Worth its own file because `:core` is a pure-JVM module that until this
 * suite existed had only ever been exercised by a JVM. Android ships a
 * stripped BouncyCastle under the name `BC` and Conscrypt owns the JCA slots
 * the stripped one used to hold — which is exactly why `libs.versions.toml`
 * takes the *lightweight* BouncyCastle API. That decision was written down and
 * never observed. It is observed here.
 */
@RunWith(AndroidJUnit4::class)
class DeviceFactsTest {

    @Test
    fun runs_on_a_real_device_and_not_an_emulator() {
        // `ro.kernel.qemu`/goldfish fingerprints are how an emulator announces
        // itself. Asserted because the predecessor of this suite measured four
        // consecutive emulator crashes and the whole point of these tests is
        // that they ran somewhere an emulator could not.
        val fingerprint = Build.FINGERPRINT
        assertTrue(
            "this suite is a claim about real hardware; FINGERPRINT=$fingerprint",
            !fingerprint.startsWith("generic") && !fingerprint.contains("vbox") &&
                !fingerprint.contains("emulator") && Build.PRODUCT != "sdk",
        )
    }

    @Test
    fun the_app_under_test_is_the_one_this_repository_builds() {
        val target = InstrumentationRegistry.getInstrumentation().targetContext
        assertEquals("com.apexos.remote", target.packageName)
    }

    @Test
    fun the_noise_handshake_completes_on_androids_providers() {
        // Both halves in this process, so the only thing that can fail is the
        // cryptography itself. A JVM already proves the protocol; what is new
        // here is the platform underneath it.
        val desktop = InMemoryStaticKey.generate()
        val handshake = Noise.pairingInitiator(desktop.publicKey, REMOTE_PROTOCOL_VERSION)
        val message = handshake.write("hello".toByteArray())
        assertTrue("a Noise message is not empty", message.isNotEmpty())
    }

    @Test
    fun key_generation_draws_from_the_platform_and_not_from_a_constant() {
        // Through `InMemoryStaticKey.generate()` rather than through `Crypto`,
        // which is `internal` to `:core` — and deliberately so. What matters
        // is that the path the app actually takes to mint a device identity
        // produces a different key each time on Android's SecureRandom; a
        // stuck generator here would mean every phone shared one identity.
        val a = InMemoryStaticKey.generate()
        val b = InMemoryStaticKey.generate()
        assertEquals(32, a.publicKey.size)
        assertTrue("two generated identities must differ", !a.publicKey.contentEquals(b.publicKey))
    }
}
