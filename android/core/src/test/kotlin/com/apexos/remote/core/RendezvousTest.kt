package com.apexos.remote.core

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertNotEquals
import org.junit.jupiter.api.Assertions.assertTrue
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

    @Test
    fun `the disclosure is the desktop's own words and not a friendlier version`() {
        // `Path::disclosure` in `apexd/apex-remote-core/src/rendezvous.rs` says
        // in its own doc why it exists once: "so the CLI and the shell page and
        // the Android app cannot each invent their own reassuring version of
        // it". An app that paraphrased would be the thing that doc warns about,
        // and nothing but this test would notice — both sides would pass their
        // own suites while telling a person two different things about who can
        // see their session.
        //
        // So the strings are read OUT of the Rust source and compared, not
        // merely looked for.
        var dir: java.io.File? = java.io.File("").absoluteFile
        var found: java.io.File? = null
        while (dir != null && found == null) {
            val candidate = java.io.File(dir, "apexd/apex-remote-core/src/rendezvous.rs")
            if (candidate.isFile) found = candidate
            dir = dir.parentFile
        }
        val rust = requireNotNull(found) {
            "rendezvous.rs was not found from ${java.io.File("").absolutePath}. This test " +
                "compares a privacy disclosure against the desktop's, so a run that could not " +
                "read it must fail rather than report that it found no drift."
        }.readText()

        // Rust wraps the long one across lines with a trailing backslash; the
        // continuation's leading whitespace is not part of the string.
        val unwrapped = Regex("\\\\\\s*\\n\\s*").replace(rust, "")
        for (path in Rendezvous.Path.entries) {
            assertTrue(
                unwrapped.contains("\"${path.disclosure()}\""),
                "the ${path.text} disclosure has drifted from the desktop's:\n" +
                    "  this app says: ${path.disclosure()}",
            )
        }
    }
}
