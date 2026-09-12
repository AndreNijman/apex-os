package com.apexos.remote.core

/**
 * What [Session] is, seen from above: a named machine that frames go to and
 * come from.
 *
 * ## Why an interface exists for a class with one implementation
 *
 * Not for layering. For the reconnect tests. Everything above this — the
 * multiplexer, the control round trip, the PTY attachment and its reconnect
 * loop — is logic with real failure modes, and every one of those failure
 * modes is reached by a connection dying at an awkward moment. Driving that
 * through a real Noise handshake over real pipes is possible, and one test
 * does exactly that; but a fake that can be told *when* to die, and how, is
 * what makes "the socket failed in the middle of a Data frame" a case rather
 * than an accident of timing.
 *
 * The rules a real implementation keeps, and a fake must keep too:
 *
 * * **One reader.** The receiving nonce is a counter, so two threads calling
 *   [receive] would each advance it past the other's message.
 * * **[send] is safe from several threads** and serialises internally, for
 *   the same reason in the other direction.
 * * **[receive] throws at end of stream** and never returns a sentinel: a
 *   connection that died must not look like a quiet one.
 */
interface FrameChannel : java.io.Closeable {
    /** The machine's name, as it gave it during the handshake. */
    val machine: String

    fun send(frame: Frame)

    fun sendData(channelId: UInt, bytes: ByteArray)

    /**
     * The next frame that is not a keepalive.
     *
     * Pings are answered and pongs are timed inside the implementation, so
     * they never surface: a caller that had to handle them would be every
     * caller, and one that ignored them would leave the desktop reporting an
     * unknown connection quality forever.
     */
    fun receive(): Frame
}
