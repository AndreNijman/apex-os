package com.apexos.remote.core

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json

/**
 * Pairing: the QR code, the token behind it, and what makes it safe.
 *
 * ## The QR code is the security boundary
 *
 * Everything else in this protocol is a key exchange between two parties that
 * already know each other. Pairing is the one moment they do not, and the
 * thing that bridges it is a human looking at a screen. The QR carries the
 * desktop's static public key and this device **pins** it: a man in the middle
 * presenting its own key makes the handshake fail rather than succeed with a
 * warning, and there is no "verify these emoji" step for anyone to click
 * through.
 *
 * Ported from `apexd/apex-remote-core/src/pairing.rs`.
 */
object Pairing {
    /**
     * The scheme a QR payload is wrapped in.
     *
     * A scheme rather than a bare blob so a camera app that resolves URIs
     * hands it to APEX Remote instead of to a browser.
     */
    const val SCHEME = "apex-remote:"

    /** The bytes in a pairing token. */
    const val TOKEN_BYTES = 32

    /**
     * How long a pairing offer stands, in milliseconds. Three minutes, set by
     * the desktop; repeated here so the device can say how long is left rather
     * than only that the offer failed.
     */
    const val OFFER_TTL_MS: Long = 3L * 60L * 1000L

    /**
     * `encodeDefaults` is on because the desktop's `serde` emits every field
     * including `null` and an absent `lan`, and a payload this end produced
     * has to be the one the desktop would have produced. `ignoreUnknownKeys`
     * is on because the desktop will grow fields and an older app must not
     * refuse to pair over one it has never heard of.
     */
    internal val json = Json {
        encodeDefaults = true
        ignoreUnknownKeys = true
        explicitNulls = true
    }

    /** Read a QR payload. */
    fun decodeOffer(text: String): PairingOffer {
        val body = text.trim().removePrefix(SCHEME)
        if (body == text.trim()) throw PairingException(PairingError.NotAnOffer)
        val raw = Base64Url.decode(body) ?: throw PairingException(PairingError.NotAnOffer)
        val offer = try {
            json.decodeFromString(PairingOffer.serializer(), raw.toString(Charsets.UTF_8))
        } catch (e: Exception) {
            throw PairingException(PairingError.Malformed(e.message ?: "not a pairing offer"))
        }
        // Checked here rather than by the caller. A caller that forgot would
        // have an offer whose key is not a key, and would find out inside the
        // handshake with an error about the crypto library.
        try {
            Device.checkKey(offer.key)
        } catch (e: DeviceException) {
            throw PairingException(PairingError.Malformed(e.message ?: "bad key"))
        }
        if (Base64Url.decode(offer.token)?.size != TOKEN_BYTES) {
            throw PairingException(PairingError.Malformed("a pairing token is $TOKEN_BYTES bytes"))
        }
        return offer
    }

    /** The text that would go in the QR code for [offer]. */
    fun encodeOffer(offer: PairingOffer): String =
        SCHEME + Base64Url.encode(
            json.encodeToString(PairingOffer.serializer(), offer).toByteArray(Charsets.UTF_8),
        )
}

/** What the QR code says. */
@Serializable
data class PairingOffer(
    /** The protocol revision, so an older app says so before it tries a handshake. */
    val v: Int,
    /** What the machine is called, for this device's own list. Display only. */
    val machine: String,
    /** The desktop's static public key, base64url. What this device pins. */
    val key: String,
    /** The one-time token, base64url. */
    val token: String,
    /**
     * Where to reach the machine on the local network, as `host:port`.
     *
     * A list because a laptop has several addresses. None of them is
     * authenticated and none needs to be: reaching the wrong address produces
     * a handshake that does not complete, not a connection to the wrong
     * machine.
     */
    val lan: List<String> = emptyList(),
    /** The relay to meet at when no LAN address works. `null` is a complete configuration. */
    val relay: String? = null,
    /** Unix milliseconds at which the offer stops being accepted. */
    @SerialName("expires_ms") val expiresMs: Long,
) {
    fun isLive(nowMs: Long): Boolean = nowMs < expiresMs

    fun ttlMs(nowMs: Long): Long = (expiresMs - nowMs).coerceAtLeast(0)

    /** The 32 raw bytes of [key]. Throws if it is not a key. */
    fun desktopPublicKey(): ByteArray = Device.checkKey(key)
}

/**
 * What this device sends inside the encrypted pairing handshake.
 *
 * Not in the clear anywhere: `Noise_NK` encrypts the first handshake message
 * to the desktop's static key, so this is readable only by the machine whose
 * QR code was scanned.
 */
@Serializable
data class PairingRequest(
    /** This device's static public key, base64url. What gets stored. */
    val key: String,
    /** What the owner should see this device called. */
    val name: String,
    /** The offer's token, base64url. */
    val token: String,
    /**
     * Whether this device holds its key behind a biometric or device
     * credential. A claim; the desktop records it and cannot check it.
     */
    @SerialName("user_verification") val userVerification: Boolean = false,
)

/** The desktop's answer, which arrives inside the finished handshake either way. */
@Serializable
data class PairingAnswer(
    val ok: Boolean,
    val device: String? = null,
    val machine: String? = null,
    val error: String? = null,
)

/** Why a pairing attempt failed. */
sealed class PairingError {
    /** The text is not an APEX Remote pairing payload at all. */
    object NotAnOffer : PairingError()

    /** It is, and it does not parse. */
    data class Malformed(val why: String) : PairingError()

    /** The offer has run out. */
    object Expired : PairingError()

    /**
     * The token does not match the standing offer.
     *
     * One variant for "wrong" and for "there is no offer", deliberately: a
     * caller that could tell them apart would have an oracle for whether
     * pairing is currently open.
     */
    object BadToken : PairingError()

    /** The device sent something that is not a key, or not a usable name. */
    data class BadDevice(val why: String) : PairingError()

    /** The desktop refused, in its own words. */
    data class Refused(val why: String) : PairingError()

    val message: String
        get() = when (this) {
            NotAnOffer -> "that is not an APEX Remote pairing code; it should begin with `${Pairing.SCHEME}`"
            is Malformed -> "the pairing code is damaged: $why"
            Expired -> "that pairing code has expired; show a new one with `apex remote pair`"
            BadToken -> "this machine is not offering to pair, or the code has already been used"
            is BadDevice -> why
            is Refused -> why
        }
}

class PairingException(val error: PairingError) : Exception(error.message)
