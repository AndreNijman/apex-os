package com.apexos.remote.core

/**
 * This device's long-term identity, expressed as *what can be done with it*
 * rather than as the bytes it is made of.
 *
 * The handshake never asks for the private key; it asks for a Diffie-Hellman
 * against a peer. That distinction is the whole point of this interface, and
 * it is what lets the Android build put the key somewhere the application
 * process cannot read:
 *
 * * [InMemoryStaticKey] holds raw bytes and is what the JVM tests use.
 * * The Android implementation holds the key wrapped by an AES key in the
 *   platform keystore with `setUserAuthenticationRequired(true)`, so [agree]
 *   is reachable only behind a biometric or device credential and the raw
 *   secret is never a `ByteArray` the app can print, serialise or leak.
 *
 * Neither the handshake nor anything else in this module has to change for the
 * second of those to exist, which is why the seam is here now, before it is
 * needed, rather than after a rewrite.
 */
interface StaticKey {
    /** The X25519 public half. Safe to print, store and put in a QR payload. */
    val publicKey: ByteArray

    /**
     * X25519 against [remotePublic]. The one operation the handshake needs.
     *
     * Implementations may block — on Android this can raise a biometric
     * prompt — and may throw if the user refuses. A refusal is a handshake
     * that does not complete, which is the correct outcome and is
     * indistinguishable to an observer from any other failure.
     */
    fun agree(remotePublic: ByteArray): ByteArray

    /** This key's base64url public half: what the desktop stores and displays. */
    fun publicKeyText(): String = Base64Url.encode(publicKey)

    /** The device id the desktop will show: the first 16 characters of the key. */
    fun deviceId(): String = Device.idFor(publicKeyText())
}

/**
 * A static key whose secret half is in this process's heap.
 *
 * Correct for tests and for a desktop; **not** what ships as the device
 * identity on Android. It exists in the shipped module anyway because the
 * pairing handshake has to generate a key before there is anywhere to put it,
 * and because a test that could not construct one could not test anything.
 */
class InMemoryStaticKey(secret: ByteArray) : StaticKey {
    init {
        require(secret.size == Crypto.DHLEN) {
            "an X25519 secret is ${Crypto.DHLEN} bytes; this one is ${secret.size}"
        }
    }

    private val secret: ByteArray = secret.copyOf()

    override val publicKey: ByteArray = Crypto.publicFromSecret(this.secret)
        get() = field.copyOf()

    override fun agree(remotePublic: ByteArray): ByteArray = Crypto.x25519(secret, remotePublic)

    /**
     * The secret, for the one caller that legitimately needs it: whatever is
     * about to wrap it and put it somewhere safe.
     *
     * A method with an unmissable name rather than a property, so that every
     * use of it is greppable and shows up in review. There is deliberately no
     * getter, no `Serializable`, and no `toString` that could reach it.
     */
    fun exportSecretForSealing(): ByteArray = secret.copyOf()

    /**
     * Redacted, always.
     *
     * Android's crash reporters format objects into reports, `Log.d` formats
     * them into logcat, and a default `toString` on a class holding a private
     * key is how a key reaches both. The public half is printed because it is
     * public and because a redaction that showed nothing would make debugging
     * a pairing impossible.
     */
    override fun toString(): String = "InMemoryStaticKey(public=${publicKeyText()}, secret=<redacted>)"

    companion object {
        fun generate(): InMemoryStaticKey = InMemoryStaticKey(Crypto.generateSecret())
    }
}
