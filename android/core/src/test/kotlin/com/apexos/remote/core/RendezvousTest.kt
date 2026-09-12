package com.apexos.remote.core

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertNotEquals
import org.junit.jupiter.api.Test

/**
 * The relay meeting point, checked against the value the desktop derives.
 *
 * Both ends compute this from the same pinned public key and nothing is
 * exchanged, so an implementation that is merely self-consistent meets nobody:
 * the device would wait at one path and the desktop at another, and the only
 * symptom would be a connection that never arrives.
 */
class RendezvousTest {
    @Test
    fun `the rendezvous id matches the one apex-remote-core derives`() {
        assertEquals(Vectors.text("rendezvous_id"), Rendezvous.idFor(Vectors.hex("desktop_public_hex")))
    }

    @Test
    fun `the rendezvous id is not the key and does not contain it`() {
        // The reason it is hashed at all: a relay operator sees every path that
        // crosses it, and a path that WAS the desktop's public key would hand
        // them the one value a device needs in order to pair.
        val key = Vectors.hex("desktop_public_hex")
        val id = Rendezvous.idFor(key)
        assertNotEquals(Base64Url.encode(key), id)
        assertFalse(Base64Url.encode(key).contains(id))
        assertEquals(22, id.length, "128 bits, base64url unpadded")
    }

    @Test
    fun `two machines do not share a meeting point`() {
        assertNotEquals(
            Rendezvous.idFor(Vectors.hex("desktop_public_hex")),
            Rendezvous.idFor(Vectors.hex("device_public_hex")),
        )
    }

    @Test
    fun `a path prefix is kept and a trailing slash is not`() {
        val id = Vectors.text("rendezvous_id")
        assertEquals("/r/$id?role=guest", Rendezvous.path("https://relay.example", id))
        assertEquals("/r/$id?role=guest", Rendezvous.path("https://relay.example/", id))
        assertEquals("/apex/r/$id?role=guest", Rendezvous.path("https://relay.example/apex", id))
        assertEquals("/apex/r/$id?role=guest", Rendezvous.path("https://relay.example/apex/", id))
        assertEquals("/r/$id?role=host", Rendezvous.path("wss://relay.example", id, Rendezvous.Role.HOST))
    }
}
