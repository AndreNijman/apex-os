package com.rimeos.remote.security

import android.os.Build
import androidx.biometric.BiometricManager
import androidx.biometric.BiometricPrompt
import androidx.fragment.app.FragmentActivity
import com.rimeos.remote.core.InMemoryStaticKey
import com.rimeos.remote.core.SecretBox
import com.rimeos.remote.core.StaticKey
import javax.crypto.Cipher
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException
import kotlinx.coroutines.suspendCancellableCoroutine

/**
 * P1-053's "app lock via biometrics or device credential", and P1-051's second
 * factor, which are the same mechanism seen from two directions.
 *
 * The lock is not a screen that hides content: it is the key itself. Unwrapping
 * the device identity requires a `Cipher` the keystore authorised, and the
 * keystore authorises one only after a `BiometricPrompt` succeeds — so an app
 * that skipped the prompt would not be an app showing secrets it should not, it
 * would be an app that cannot open a session at all. That is the difference
 * between a lock and a curtain, and it is why there is no `isUnlocked` flag
 * anywhere in this file for something to get wrong.
 */
object AppLock {
    /**
     * Which authenticators this build will accept, for the API level it is on.
     *
     * `BIOMETRIC_STRONG` and never `BIOMETRIC_WEAK`: a weak biometric cannot
     * release a keystore key, so accepting one would produce a prompt that
     * succeeds and then a `Cipher` that still refuses.
     *
     * `DEVICE_CREDENTIAL` only from API 30. Below that, `BiometricPrompt`
     * refuses the combination with a `CryptoObject` in so many words —
     * *"Crypto-based authentication is not supported for device credential
     * prior to API 30."* — and every authentication this app performs carries a
     * `CryptoObject`, because the cipher is the whole point. This branch is the
     * mirror of the one in [KeystoreSecretBox.forDevice]; if one moves, both do.
     */
    fun authenticators(): Int =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            BiometricManager.Authenticators.BIOMETRIC_STRONG or
                BiometricManager.Authenticators.DEVICE_CREDENTIAL
        } else {
            BiometricManager.Authenticators.BIOMETRIC_STRONG
        }

    /** Whether this device can gate a key at all. */
    fun canAuthenticate(manager: BiometricManager): Boolean =
        manager.canAuthenticate(authenticators()) == BiometricManager.BIOMETRIC_SUCCESS

    /**
     * Show the prompt, and return the device identity it unlocked.
     *
     * The cipher is initialised **before** the prompt and carried through it,
     * because a per-use keystore key authorises one cipher rather than a period
     * of time. The unwrap then happens on the object the prompt hands back.
     */
    suspend fun unlock(
        activity: FragmentActivity,
        box: KeystoreSecretBox,
        sealed: ByteArray,
        machineName: String,
    ): StaticKey {
        if (!box.authenticationIsRequired) return InMemoryStaticKey(box.open(sealed))
        val authorised = prompt(
            activity,
            box.beginOpen(sealed),
            title = "Unlock Rime Remote",
            subtitle = "Connecting to $machineName",
        )
        return InMemoryStaticKey(box.finishOpen(authorised, sealed))
    }

    /**
     * What one unlock prompt produced: the app is open, and — when it could be
     * — one machine's key is unwrapped.
     */
    class Unlocked(
        /** The unwrapped key for the machine the prompt named, or null. */
        val identity: StaticKey?,
        /**
         * Why the key could not be unwrapped, when it could not, for the
         * screen to say once the person tries that machine. Null on success
         * and when no key was attempted.
         */
        val keyFailure: Throwable?,
    )

    /**
     * The front door and the key in ONE prompt, where the key allows it.
     *
     * ## Why this is not weaker than two prompts
     *
     * The two prompts it replaces were [confirmPresence] — no `CryptoObject`,
     * so it authorises nothing and is, in its own words, a curtain — and then
     * [unlock] for the machine the person tapped, which carries the cipher.
     * Both accept exactly the same authenticators ([authenticators]). So a
     * prompt that carries the cipher proves everything the curtain did and
     * more: it cannot succeed without the person, and it cannot be satisfied
     * by anything the curtain would not also accept.
     *
     * What does NOT change is the key rule, and that is the reason this is
     * one machine and not all of them. Each machine's wrapping key is gated
     * per use (`KeystoreSecretBox`: timeout 0), so one authentication
     * authorises exactly ONE cipher, and therefore one unwrap. A second
     * machine still needs its own prompt when it is first used, and there is
     * no way to batch them without making the keys time-bound — a window in
     * which a phone taken out of a hand works — which this design refuses.
     * The unwrapped key goes where it always went: memory, never storage.
     *
     * ## When it falls back to the curtain
     *
     * No machine to name ([box] null), or a key that cannot start an unwrap
     * at all — invalidated by a new fingerprint, a keystore that was wiped.
     * Then the app still opens behind [confirmPresence], and the key's
     * problem is reported when that machine is used, where the screen can
     * say "forget it and pair again". A key that is simply UNGATED (a phone
     * paired with no screen lock) unwraps without a prompt, and the curtain
     * is still shown, because a curtain is what this door is for.
     */
    suspend fun unlockApp(
        activity: FragmentActivity,
        box: KeystoreSecretBox?,
        sealed: ByteArray?,
        machineName: String?,
    ): Unlocked {
        if (box == null || sealed == null || machineName == null) {
            confirmPresence(activity)
            return Unlocked(null, null)
        }
        if (!box.authenticationIsRequired) {
            val identity = runCatching { InMemoryStaticKey(box.open(sealed)) }
            confirmPresence(activity)
            return Unlocked(identity.getOrNull(), identity.exceptionOrNull())
        }
        val cipher = try {
            box.beginOpen(sealed)
        } catch (e: Exception) {
            // Before any prompt: the key cannot even begin. The door still
            // opens; the machine's problem is its own.
            confirmPresence(activity)
            return Unlocked(null, e)
        }
        // AppLockRefused (cancelled, too many attempts) propagates: that is
        // the person saying no, and the app stays locked.
        val authorised = prompt(
            activity,
            cipher,
            title = "Unlock Rime Remote",
            subtitle = "Connecting to $machineName",
        )
        // The person is present — the prompt succeeded — so the app is open
        // whatever happens next. A key whose ciphertext no longer matches
        // (a keystore wiped and a fresh key created in its place) fails here,
        // after the prompt, and is reported for that machine.
        return try {
            Unlocked(InMemoryStaticKey(box.finishOpen(authorised, sealed)), null)
        } catch (e: Exception) {
            Unlocked(null, e)
        }
    }

    /**
     * A box authorised to seal exactly once, for pairing.
     *
     * Sealing is gated as tightly as opening — `KeyGenParameterSpec` restricts
     * *all* secret-key operations, and an AES key is a secret key — so the new
     * device identity cannot be wrapped without the user being present either.
     * That is not a tax: it is what makes `user_verification: true` in the
     * pairing request a statement about something that just happened rather
     * than a promise about something that might.
     *
     * One use, and it says so when asked for a second: a `Cipher` a per-use key
     * authorised is spent after its `doFinal`, and a box that pretended
     * otherwise would fail somewhere further away from the cause.
     */
    suspend fun authoriseSeal(
        activity: FragmentActivity,
        box: KeystoreSecretBox,
        machineName: String,
    ): SecretBox {
        if (!box.authenticationIsRequired) return box
        val authorised = prompt(
            activity,
            box.beginSeal(),
            title = "Confirm it is you",
            subtitle = "Pairing this phone with $machineName",
        )
        return OneShotSeal(box, authorised)
    }

    /**
     * The lock on the front door: a prompt with no cipher behind it.
     *
     * Deliberately the weakest thing in this file, and named so that nobody
     * mistakes it for the strong one. It carries no `CryptoObject`, so it
     * authorises nothing and unlocks no key — all it does is stop a phone that
     * was picked up off a table from showing which machines exist and what they
     * are called. That is worth having and it is not what protects the device
     * identity; [unlock] is.
     *
     * Said plainly because the failure mode of a lock like this is that a later
     * change quietly starts relying on it — a `isUnlocked` flag consulted
     * instead of a prompt, and the gate becomes a boolean somebody can flip.
     */
    suspend fun confirmPresence(activity: FragmentActivity): Unit =
        suspendCancellableCoroutine { continuation ->
            val info = BiometricPrompt.PromptInfo.Builder()
                .setTitle("Rime Remote")
                .setSubtitle("Unlock to see your computers")
                .setAllowedAuthenticators(authenticators())
                .apply {
                    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) setNegativeButtonText("Cancel")
                }
                .build()
            val prompt = BiometricPrompt(
                activity,
                androidx.core.content.ContextCompat.getMainExecutor(activity),
                object : BiometricPrompt.AuthenticationCallback() {
                    override fun onAuthenticationSucceeded(
                        result: BiometricPrompt.AuthenticationResult,
                    ) = continuation.resume(Unit)

                    override fun onAuthenticationError(code: Int, message: CharSequence) {
                        continuation.resumeWithException(AppLockRefused(message.toString(), code))
                    }
                },
            )
            prompt.authenticate(info)
            continuation.invokeOnCancellation { prompt.cancelAuthentication() }
        }

    /**
     * What [authoriseSeal] returns: the sealing half of a box, already through
     * a prompt, and an unambiguous refusal for everything else.
     */
    private class OneShotSeal(
        private val box: KeystoreSecretBox,
        private val authorised: Cipher,
    ) : SecretBox {
        private var spent = false

        override val describe: String get() = box.describe

        override fun seal(plaintext: ByteArray): ByteArray {
            check(!spent) {
                "this box was authorised for one seal and has already been used; " +
                    "a per-use keystore key needs a fresh prompt"
            }
            spent = true
            return box.finishSeal(authorised, plaintext)
        }

        override fun open(ciphertext: ByteArray): ByteArray =
            throw UnsupportedOperationException(
                "this box was authorised to seal a new device key, not to open one",
            )
    }

    private suspend fun prompt(
        activity: FragmentActivity,
        cipher: Cipher,
        title: String,
        subtitle: String,
    ): Cipher = suspendCancellableCoroutine { continuation ->
        val info = BiometricPrompt.PromptInfo.Builder()
            .setTitle(title)
            // Named, because the whole point of the gate is that the person
            // holding the phone knows which machine is about to be reachable.
            .setSubtitle(subtitle)
            .setAllowedAuthenticators(authenticators())
            .apply {
                // Below API 30 the allowed set is biometric only, and a prompt
                // with no negative button and no device credential has no way
                // out at all. `setNegativeButtonText` is refused *with*
                // DEVICE_CREDENTIAL, so this is the same branch again.
                if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) setNegativeButtonText("Cancel")
            }
            .build()
        val prompt = BiometricPrompt(
            activity,
            androidx.core.content.ContextCompat.getMainExecutor(activity),
            object : BiometricPrompt.AuthenticationCallback() {
                override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                    val authorised = result.cryptoObject?.cipher
                    if (authorised == null) {
                        // Never expected, and refused rather than worked
                        // around: falling back to the un-authorised cipher
                        // would turn the lock into a curtain.
                        continuation.resumeWithException(
                            IllegalStateException("the prompt returned no authorised cipher"),
                        )
                    } else {
                        continuation.resume(authorised)
                    }
                }

                override fun onAuthenticationError(code: Int, message: CharSequence) {
                    continuation.resumeWithException(AppLockRefused(message.toString(), code))
                }

                // `onAuthenticationFailed` is a finger the sensor did not
                // recognise, not a refusal. The prompt stays up and the user
                // tries again, so there is nothing to resume.
            },
        )
        prompt.authenticate(info, BiometricPrompt.CryptoObject(cipher))
        continuation.invokeOnCancellation { prompt.cancelAuthentication() }
    }
}

/** The user declined, or the system refused. */
class AppLockRefused(message: String, val code: Int) : Exception(message)
