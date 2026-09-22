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

    /**
     * Which way a connection reached the machine, and what to tell the person
     * holding the phone about it.
     *
     * Ported from `Path` in `apexd/apex-remote-core/src/rendezvous.rs`, and the
     * disclosure strings are **verbatim** from `Path::disclosure` there. That
     * function's own doc says why: it is written once "so the CLI and the shell
     * page and the Android app cannot each invent their own reassuring version
     * of it", and an app that paraphrased would be the thing it warns about.
     * `RendezvousTest` reads the Rust source and compares.
     */
    enum class Path(val text: String) {
        /** A direct TCP connection on the local network. */
        LAN("lan"),

        /** Through a relay. Slower, and observable by its operator. */
        RELAY("relay"),
        ;

        /** What the user is told about privacy on this path. */
        fun disclosure(): String = when (this) {
            LAN -> "direct on this network; nothing leaves it and no third party is involved"
            RELAY -> "through a relay, which carries encrypted bytes it cannot read but does " +
                "see both addresses, when you connect and how much data moves"
        }
    }

    /**
     * The order a client tries paths in.
     *
     * LAN first, always. It is faster, it involves nobody else, and when it
     * works the relay never learns the session happened at all. A client that
     * raced both and took whichever answered first would leak a rendezvous
     * connection every time, including on the network where it was
     * unnecessary.
     */
    val PREFERENCE = listOf(Path.LAN, Path.RELAY)

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
