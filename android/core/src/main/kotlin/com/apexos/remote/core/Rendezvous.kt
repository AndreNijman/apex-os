package com.apexos.remote.core

/**
 * Finding a desktop through a relay without telling the relay who it is.
 *
 * Ported from `apexd/apex-remote-core/src/rendezvous.rs`.
 */
object Rendezvous {
    /**
     * The domain separator mixed into the hash, so that this hash of a public
     * key can never collide with some other protocol's hash of the same key.
     */
    private val DOMAIN = "apex-remote/rendezvous/v1".toByteArray(Charsets.US_ASCII)

    /**
     * The meeting-point name for a machine with this static public key.
     *
     * Hashed rather than used raw: the relay operator sees every path that
     * crosses it, and a path that *was* the desktop's public key would hand
     * them the one value a device needs to pair. Deterministic, so nothing has
     * to be exchanged and no state has to be stored on the relay for a device
     * to find its desktop again after a reinstall.
     */
    fun idFor(desktopPublic: ByteArray): String {
        require(desktopPublic.size == Crypto.DHLEN) {
            "a desktop key is ${Crypto.DHLEN} bytes; this one is ${desktopPublic.size}"
        }
        return Base64Url.encode(Crypto.sha256(DOMAIN, desktopPublic).copyOf(16))
    }

    /** Which end of a rendezvous a connection is. A device is always the guest. */
    enum class Role(val text: String) {
        HOST("host"),
        GUEST("guest"),
    }

    /**
     * The relay path for a rendezvous: `{prefix}/r/{id}?role=guest`.
     *
     * [base] is the relay's base URL as the QR payload carried it; any path it
     * has is kept, and a trailing slash is not.
     */
    fun path(base: String, id: String, role: Role = Role.GUEST): String {
        val withoutScheme = base.substringAfter("://", base)
        val prefix = withoutScheme.substringAfter('/', "").trimEnd('/')
        val leading = if (prefix.isEmpty()) "" else "/$prefix"
        return "$leading/r/$id?role=${role.text}"
    }
}
