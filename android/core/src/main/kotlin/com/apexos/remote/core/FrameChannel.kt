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

    /**
     * `System.nanoTime()` when a frame last arrived — **keepalives included**
     * — or `null` for a channel that does not track it.
     *
     * ## Why this cannot be computed above
     *
     * `apex-remoted` sends a `Ping` every fifteen seconds and those pings are
     * the *only* traffic an idle session has. [receive] answers them and never
     * returns them, which is right: a caller that had to handle keepalives
     * would be every caller. But it means a layer above this sees exactly the
     * same thing on a healthy idle terminal and on a connection that has
     * silently gone away — no frames at all — and those two need telling
     * apart, because the second one is a phone that walked into a lift.
     *
     * A dead TCP connection does not announce itself. `soTimeout` is
     * deliberately zero after the handshake (`PairingService.connect` says
     * why: a PTY with nobody typing produces no bytes for hours), so a socket
     * whose peer vanished without a FIN will sit in `read` until the kernel's
     * own keepalive gives up, which on Android is measured in hours.
     *
     * ## `null` refuses rather than lies
     *
     * A default of "zero, meaning fresh" would make a watchdog over a channel
     * that does not track this silently watch nothing — a reconnect test that
     * passes because nothing ever disconnected. `null` makes the watchdog
     * refuse to arm and say so.
     */
    val lastFrameNanos: Long? get() = null
}
