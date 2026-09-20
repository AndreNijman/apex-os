package com.apexos.remote.core

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.assertThrows
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.io.PipedInputStream
import java.io.PipedOutputStream

/**
 * The app and the OS are updated by different people at different times, so
 * they are routinely at different ages. These are the tests that say what
 * happens then.
 *
 * The failure they exist to prevent is a real one this code shipped with. The
 * protocol revision is hashed into the Noise prologue and never transmitted, so
 * a desktop refuses a revision it does not speak by closing the socket without
 * replying — which is byte-for-byte what it does to a device that has been
 * revoked. [Client.openSession] therefore reported every version mismatch as
 * *"it is not paired, or it has been revoked"*, and bumping
 * `REMOTE_PROTOCOL_VERSION` would have told every installed phone it had been
 * thrown out.
 *
 * Two things fix it, and both are asserted here rather than described in a
 * comment: the client tries **every** revision it speaks before giving up, and
 * when they are all refused it says so in a sentence that names the version as
 * a possible cause instead of asserting the revocation.
 *
 * Everything below drives a real Noise handshake against a real responder in a
 * thread. Nothing greps a constant.
 */
class VersionWindowTest {
    private val desktop = InMemoryStaticKey(ByteArray(Crypto.DHLEN) { 7 })
    private val device = InMemoryStaticKey(ByteArray(Crypto.DHLEN) { 9 })

    /** One attempt's streams, and whether the client closed them. */
    private class Leg(val input: InputStream, val output: OutputStream) {
        var closed = false
    }

    /**
     * A desktop that hangs up without replying — what `apex-remoted` does to
     * both a revoked device and a revision it does not speak
     * (`serve.rs`: the handshake read fails and the connection is dropped).
     */
    private fun refusingDesktop() = Leg(ByteArrayInputStream(ByteArray(0)), ByteArrayOutputStream())

    /** A desktop that really completes a session handshake at [version]. */
    private fun acceptingDesktop(version: Int, machineName: String): Pair<Leg, Thread> {
        val toDesktop = PipedOutputStream()
        val desktopReads = PipedInputStream(toDesktop, 1 shl 16)
        val toDevice = PipedOutputStream()
        val deviceReads = PipedInputStream(toDevice, 1 shl 16)
        val thread = Thread {
            check(desktopReads.read() == Transport.HELLO_SESSION.toInt())
            val handshake = Noise.sessionResponder(desktop, version)
            handshake.read(Transport.readMessage(desktopReads))
            Transport.writeMessage(toDevice, handshake.write(machineName.toByteArray(Charsets.UTF_8)))
            toDevice.flush()
        }
        // A DAEMON thread, and not as a detail. A responder blocked on a read
        // that will never be satisfied — which is exactly what happens when
        // the client is broken — keeps a non-daemon thread alive, and the
        // forked test JVM then never exits. That turns a failing test into a
        // hung build, which is strictly worse: a red test tells you what is
        // wrong and a hang tells you nothing. Measured: a mutation of
        // `Client.pair` hung this suite for twelve minutes before this line.
        thread.isDaemon = true
        thread.start()
        return Leg(deviceReads, toDesktop) to thread
    }

    @Test
    fun `an app falls back to an older revision when its newest is refused`() {
        // The window is {2, 1} and the desktop speaks 1 — an app that has been
        // updated talking to a machine that has not. Before the window existed
        // this was a dead phone.
        val legs = mutableListOf<Leg>()
        var responder: Thread? = null
        val attached = Client.openSessionAcrossVersions(
            identity = device,
            desktopPublic = desktop.publicKey,
            versions = listOf(2, 1),
            connect = {
                val leg = if (legs.isEmpty()) {
                    refusingDesktop()
                } else {
                    val (accepting, thread) = acceptingDesktop(version = 1, machineName = "l16")
                    responder = thread
                    accepting
                }
                legs += leg
                leg
            },
            streams = { it.input to it.output },
            abandon = { it.closed = true },
        )

        assertEquals(2, legs.size, "the app gave up without trying the revision the desktop speaks")
        assertEquals(1, attached.version, "the session was not attributed to the revision that worked")
        assertEquals("l16", attached.session.machine)
        // The refused attempt owns nothing and must be closed, or a phone that
        // keeps retrying leaks one descriptor per attempt.
        assertTrue(legs[0].closed, "the refused attempt's connection was left open")
        assertFalse(legs[1].closed, "the connection carrying the live session was closed")
        responder?.join(5_000)
    }

    @Test
    fun `the newest revision is tried first, so an up-to-date pair pays for one connection`() {
        val legs = mutableListOf<Leg>()
        var responder: Thread? = null
        val attached = Client.openSessionAcrossVersions(
            identity = device,
            desktopPublic = desktop.publicKey,
            versions = listOf(2, 1),
            connect = {
                val (accepting, thread) = acceptingDesktop(version = 2, machineName = "l16")
                responder = thread
                accepting.also { legs += it }
            },
            streams = { it.input to it.output },
            abandon = { it.closed = true },
        )
        assertEquals(1, legs.size, "a desktop on the newest revision still cost more than one dial")
        assertEquals(2, attached.version)
        responder?.join(5_000)
    }

    @Test
    fun `when every revision is refused the message names both causes and the window`() {
        val legs = mutableListOf<Leg>()
        val refused = assertThrows<SessionRefused> {
            Client.openSessionAcrossVersions(
                identity = device,
                desktopPublic = desktop.publicKey,
                versions = listOf(2, 1),
                connect = { refusingDesktop().also { legs += it } },
                streams = { it.input to it.output },
                abandon = { it.closed = true },
            )
        }
        val said = refused.message ?: ""
        // Both causes, because the phone genuinely cannot tell them apart. The
        // old message asserted the revocation, which is what made a protocol
        // bump look like a mass revocation.
        assertTrue(said.contains("unpaired or revoked"), said)
        assertTrue(
            said.contains("no longer speak the same APEX Remote protocol"),
            "the version was never mentioned, which is the whole defect: $said",
        )
        // The app's own window, so the user has something to compare against.
        assertTrue(said.contains("v1 to v2"), said)
        // …and where to read the other side's. A message that names a problem
        // and no way to look at it is the spinning pairing screen with words on.
        assertTrue(said.contains("apex remote status"), said)

        assertEquals(2, legs.size, "not every revision was tried before giving up")
        assertTrue(legs.all { it.closed }, "a refused attempt left its connection open")
    }

    @Test
    fun `a failure that is not a refusal is raised rather than retried`() {
        // An unreachable address is not a version problem, and retrying it once
        // per revision would turn one honest error into a handful of
        // misleading ones — and would multiply the wait the user sits through.
        var connects = 0
        assertThrows<IOException> {
            Client.openSessionAcrossVersions<Leg>(
                identity = device,
                desktopPublic = desktop.publicKey,
                versions = listOf(3, 2, 1),
                connect = { connects++; throw IOException("no route to host") },
                streams = { it.input to it.output },
                abandon = { it.closed = true },
            )
        }
        assertEquals(1, connects, "a connection failure was retried at another revision")
    }

    /**
     * A connection that dies between being made and being written to.
     *
     * The failure lands on the very first `write`, before the handshake has
     * sent anything — so it is neither a connection that could not be made nor
     * a desktop that hung up on a handshake. It is the third case, and it is
     * the one that decides whether the retry loop catches everything or only a
     * refusal.
     */
    private fun deadOnWriteDesktop(): Leg = Leg(
        ByteArrayInputStream(ByteArray(0)),
        object : OutputStream() {
            override fun write(b: Int): Unit = throw IOException("broken pipe")
            override fun write(b: ByteArray, off: Int, len: Int): Unit = throw IOException("broken pipe")
        },
    )

    @Test
    fun `a connection that dies on the first write is reported once, not once per revision`() {
        // The gap this closes: the "connection failure" test above throws from
        // `connect`, which never reaches the retry decision at all. This one
        // fails INSIDE the attempt with something that is not a refusal. A
        // loop that retried it would spend three dials and then report a
        // protocol problem for a network one.
        val legs = mutableListOf<Leg>()
        assertThrows<IOException> {
            Client.openSessionAcrossVersions(
                identity = device,
                desktopPublic = desktop.publicKey,
                versions = listOf(3, 2, 1),
                connect = { deadOnWriteDesktop().also { legs += it } },
                streams = { it.input to it.output },
                abandon = { it.closed = true },
            )
        }
        assertEquals(1, legs.size, "a dead connection was retried at every protocol revision")
        assertTrue(legs[0].closed, "the failed attempt's connection was left open")
    }

    @Test
    fun `a client that speaks no revision is a programming error, not a refusal`() {
        assertThrows<IllegalArgumentException> {
            Client.openSessionAcrossVersions<Leg>(
                identity = device,
                desktopPublic = desktop.publicKey,
                versions = emptyList(),
                connect = { error("connect must never be called for an empty window") },
                streams = { it.input to it.output },
                abandon = { },
            )
        }
    }

    // ── Pairing ──────────────────────────────────────────────────────────────

    private fun offer(v: Int) = PairingOffer(
        v = v,
        machine = "l16",
        key = Base64Url.encode(desktop.publicKey),
        token = Base64Url.encode(ByteArray(Pairing.TOKEN_BYTES) { 5 }),
        lan = listOf("192.168.1.10:7717"),
        relay = null,
        expiresMs = Long.MAX_VALUE,
    )

    @Test
    fun `pairing accepts a revision inside the window and handshakes at the desktop's`() {
        // The window prefers 2; the desktop speaks 1 and says so in the offer.
        // If `pair` handshook at its own preferred revision the prologue would
        // differ and the handshake could not complete, so a pairing that
        // finishes IS the assertion that the offer's revision was used.
        val toDesktop = PipedOutputStream()
        val desktopReads = PipedInputStream(toDesktop, 1 shl 16)
        val toDevice = PipedOutputStream()
        val deviceReads = PipedInputStream(toDevice, 1 shl 16)
        val responder = Thread {
            check(desktopReads.read() == Transport.HELLO_PAIR.toInt())
            val handshake = Noise.pairingResponder(desktop, 1)
            handshake.read(Transport.readMessage(desktopReads))
            Transport.writeMessage(
                toDevice,
                handshake.write(
                    """{"ok":true,"device":"pixel-8","machine":"l16"}""".toByteArray(Charsets.UTF_8),
                ),
            )
            toDevice.flush()
        }
        responder.isDaemon = true
        responder.start()

        val answer = Client.pair(
            input = deviceReads,
            output = toDesktop,
            offer = offer(v = 1),
            identity = device,
            deviceName = "pixel-8",
            userVerification = true,
            versions = listOf(2, 1),
        )
        assertTrue(answer.ok)
        assertEquals("l16", answer.machine)
        responder.join(5_000)
        assertFalse(responder.isAlive, "the desktop half of the pairing never finished")
    }

    @Test
    fun `pairing refuses a revision outside the window and names both sides`() {
        val output = ByteArrayOutputStream()
        val thrown = assertThrows<PairingException> {
            Client.pair(
                input = ByteArrayInputStream(ByteArray(0)),
                output = output,
                offer = offer(v = 5),
                identity = device,
                deviceName = "pixel-8",
                userVerification = true,
                versions = listOf(2, 1),
            )
        }
        val said = thrown.error.message
        assertTrue(thrown.error is PairingError.Malformed, "${thrown.error}")
        assertTrue(said.contains("v5"), said)
        assertTrue(said.contains("v1 to v2"), said)
        assertTrue(said.contains("apex update"), "the user was not told how to fix it: $said")
        // Still the property the old strict check had: a refused pairing must
        // not burn the owner's one-time offer.
        assertEquals(0, output.size(), "a refused pairing still opened a connection")
    }

    // ── The shipped window ───────────────────────────────────────────────────

    @Test
    fun `the shipped window is newest first and contains the revision this build speaks`() {
        val window = SUPPORTED_REMOTE_PROTOCOL_VERSIONS
        assertTrue(window.isNotEmpty(), "a client that speaks nothing cannot connect to anything")
        assertTrue(
            REMOTE_PROTOCOL_VERSION in window,
            "this build speaks v$REMOTE_PROTOCOL_VERSION but does not list it: $window",
        )
        // Order is load-bearing: `openSessionAcrossVersions` tries the list as
        // given, so a window that was not newest-first would make an
        // up-to-date pair dial twice on every connection.
        assertEquals(window.sortedDescending(), window, "the window is not newest-first: $window")
        assertEquals(window.distinct(), window, "a repeated revision would be dialled twice: $window")
        assertEquals(
            REMOTE_PROTOCOL_VERSION,
            window.max(),
            "this build prefers a revision it does not itself speak",
        )
    }

    @Test
    fun `a window reads the way a person would say it`() {
        assertEquals("v1", describeVersionWindow(listOf(1)))
        assertEquals("v1 to v2", describeVersionWindow(listOf(2, 1)))
        assertEquals("v3 to v7", describeVersionWindow(listOf(7, 5, 3)))
    }
}
