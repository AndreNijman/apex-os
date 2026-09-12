package com.apexos.remote.core

/**
 * The rules the desktop applies to a device, ported so that this end refuses
 * the same things rather than discovering them one round trip later.
 *
 * `apexd/apex-remote-core/src/device.rs` is the specification.
 */
object Device {
    /**
     * The id a given public key gets: the first 16 characters of the key's own
     * base64url text.
     *
     * Not a hash — the key is already uniformly random 32 bytes — and an id
     * that is a literal prefix of the key can be checked by eye against a
     * record. Nothing authenticates on the id; the desktop looks up the full
     * key.
     */
    fun idFor(publicKeyText: String): String = publicKeyText.take(16)

    /** A device name may be at most this many characters. */
    const val MAX_NAME = 64

    /** The 32 raw bytes of a base64url key, or the reason it is not one. */
    fun checkKey(key: String): ByteArray {
        val raw = Base64Url.decode(key)
            ?: throw DeviceException("'$key' is not base64url")
        if (raw.size != Crypto.DHLEN) {
            throw DeviceException("a device key is ${Crypto.DHLEN} bytes; this one decodes to ${raw.size}")
        }
        return raw
    }

    /**
     * A displayable device name, trimmed, or the reason it is not one.
     *
     * The same rule `apex-agentd` applies to an actor id, and for the same
     * reason: this name becomes that actor, and it is printed on the prompt a
     * human reads before approving a root operation. A device called
     * `phone\r\nAPPROVED` is a display attack, and it must be refused **here**
     * as well as on the desktop — a client that sent one would burn the
     * owner's pairing offer to learn that.
     */
    fun checkName(name: String): String {
        val trimmed = name.trim()
        if (trimmed.isEmpty()) throw DeviceException("a device needs a name")
        if (trimmed.codePointCount(0, trimmed.length) > MAX_NAME) {
            throw DeviceException(
                "a device name may be at most $MAX_NAME characters; this one is " +
                    trimmed.codePointCount(0, trimmed.length),
            )
        }
        if (trimmed.any { Character.isISOControl(it) }) {
            throw DeviceException(
                "a device name may not contain control characters: it is printed on the prompt a " +
                    "human reads before approving",
            )
        }
        return trimmed
    }
}

class DeviceException(message: String) : Exception(message)
