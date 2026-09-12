package com.apexos.remote.pairing

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.assertThrows

/**
 * The one piece of `:app` that is pure enough to test on a JVM, and it is the
 * piece most likely to be wrong.
 *
 * `apex-remoted` writes IPv6 addresses in brackets, and `net.rs` excludes
 * link-local addresses precisely because they cannot be written down usefully.
 * A parser that reached for `substringAfterLast(':')` would take the last group
 * of an unbracketed v6 address and call it a port — on a network where v4
 * happens to work, that bug is invisible until somebody's phone is on v6-only
 * mobile data.
 *
 * No device, no emulator, no Android class: this runs in `:app:testDebugUnitTest`
 * on the same JVM as `:core`'s suite.
 */
class PairingServiceTest {
    private val service = PairingService()

    @Test
    fun `an ordinary host and port split where they should`() {
        assertEquals("192.168.1.10" to 7717, service.splitHostPort("192.168.1.10:7717"))
        assertEquals("l16.local" to 9000, service.splitHostPort("l16.local:9000"))
    }

    @Test
    fun `a bracketed IPv6 literal keeps its colons and loses its brackets`() {
        assertEquals(
            "2001:db8::1" to 7717,
            service.splitHostPort("[2001:db8::1]:7717"),
        )
        // Brackets with no port at all: the address is still an address.
        assertEquals("2001:db8::1" to 7717, service.splitHostPort("[2001:db8::1]"))
    }

    @Test
    fun `an address with no port gets the daemon's own`() {
        assertEquals("192.168.1.10" to 7717, service.splitHostPort("192.168.1.10"))
    }

    @Test
    fun `a port that is not a number is not silently a port`() {
        // `toIntOrNull` falling back to the default is deliberate: an address
        // the desktop wrote badly should still be tried, because the handshake
        // is what decides whether the far end is the right machine. What must
        // not happen is a crash, or a port of 0.
        assertEquals("l16.local" to 7717, service.splitHostPort("l16.local:ssh"))
    }

    @Test
    fun `an unclosed bracket is refused rather than guessed at`() {
        assertThrows<IllegalArgumentException> { service.splitHostPort("[2001:db8::1:7717") }
    }

    @Test
    fun `an unbracketed IPv6 address does not have its last group eaten`() {
        // The bug this parser exists to avoid, and it was live until this test
        // was written: `lastIndexOf(':')` on `2001:db8::1` yields host
        // `2001:db8:` and port 1. Without brackets there is no way to tell a
        // port from a final group, so the whole string is the host and the
        // default port is used — a connection that fails honestly, rather than
        // one aimed at port 1 of a truncated address.
        assertEquals("2001:db8::1" to 7717, service.splitHostPort("2001:db8::1"))
        assertEquals("fe80::1%wlan0" to 7717, service.splitHostPort("fe80::1%wlan0"))
    }
}
