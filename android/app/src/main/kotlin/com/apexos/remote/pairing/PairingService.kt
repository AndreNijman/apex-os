package com.apexos.remote.pairing

import com.apexos.remote.core.Client
import com.apexos.remote.core.Device
import com.apexos.remote.core.InMemoryStaticKey
import com.apexos.remote.core.MachineStore
import com.apexos.remote.core.PairedMachine
import com.apexos.remote.core.PairingAnswer
import com.apexos.remote.core.PairingOffer
import com.apexos.remote.core.SecretBox
import com.apexos.remote.core.Session
import com.apexos.remote.core.StaticKey
import java.net.InetSocketAddress
import java.net.Socket
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * Pairing and connecting, over a real socket, off the main thread.
 *
 * `:core` is generic over streams because the LAN leg is TCP and the relay leg
 * is a WebSocket carrying the same bytes; this is the TCP half. Everything here
 * runs on [Dispatchers.IO] — a socket touched from the main thread is an
 * immediate `NetworkOnMainThreadException`, and it is a crash that never
 * happens on a laptop running the unit tests.
 */
class PairingService {
    /**
     * Pair with the machine whose QR code produced [offer], and return the
     * record to store.
     *
     * The addresses in the offer are tried in order and none of them is
     * authenticated — none needs to be. Reaching the wrong one produces a
     * handshake that does not complete, not a connection to the wrong machine,
     * so "try them all" is safe here in a way it would not be in a protocol
     * that trusted an address.
     */
    suspend fun pair(
        offer: PairingOffer,
        deviceName: String,
        boxFor: suspend (deviceId: String) -> GatedBox,
        nowMs: Long,
    ): PairedMachine {
        // The identity is generated here and not by the caller, because "a
        // fresh key per machine" is a property of pairing rather than a choice
        // a screen should be able to get wrong. The box, on the other hand, is
        // the caller's: the keystore alias is derived from the device id, which
        // does not exist until this line has run, and on Android obtaining the
        // box raises a biometric prompt that must happen on the main thread.
        // Hence a function of the id rather than a value — and hence a `suspend`
        // one.
        val identity = InMemoryStaticKey.generate()

        // The box FIRST, and the ordering is the whole point rather than a
        // style. Obtaining it raises the biometric prompt, and a prompt the
        // user cancels throws. If that happened after the handshake, the
        // desktop would already have redeemed the token and written a device
        // row whose private key had just died with the exception — a machine
        // paired with a phone that can never connect, cleared only by hand with
        // `apex remote revoke`. Cancelling here costs nothing at all.
        //
        // It is also what makes `user_verification` honest. The flag says a
        // second factor exists on this device, the desktop stores it against
        // the device and consults it when an approval needs one, and it is
        // taken from the box that was just unlocked rather than from a question
        // asked of the system beforehand.
        val gated = boxFor(identity.deviceId())
        val answer = handshake(offer, identity, deviceName, gated.userVerification)
        // The authorised cipher is still good here: a per-use keystore key
        // authorises an *operation*, not a span of time, so the seconds the
        // handshake took do not expire it.
        return MachineStore.record(identity, offer, answer, gated.box, nowMs)
    }

    private suspend fun handshake(
        offer: PairingOffer,
        identity: InMemoryStaticKey,
        deviceName: String,
        userVerification: Boolean,
    ): PairingAnswer = withContext(Dispatchers.IO) {
        var lastFailure: Exception? = null
        for (address in offer.lan) {
            val socket = try {
                connect(address)
            } catch (e: Exception) {
                lastFailure = e
                continue
            }
            // Only a *connection* failure moves on to the next address. Once a
            // socket is open the exception from inside it propagates, and that
            // is deliberate: a refusal, a spent token or a machine that could
            // not prove it holds the key from the QR code are all answers, and
            // trying the same desktop again on its other IP would produce the
            // same answer while burning the offer a second time.
            socket.use {
                return@withContext Client.pair(
                    input = it.getInputStream(),
                    output = it.getOutputStream(),
                    offer = offer,
                    identity = identity,
                    deviceName = deviceName,
                    userVerification = userVerification,
                )
            }
        }
        throw NoRouteToMachine(
            "none of the addresses in the pairing code answered: ${offer.lan.joinToString()}" +
                if (offer.relay != null) " (and no relay client has been written yet)" else "",
            lastFailure,
        )
    }

    /** Open a session with a machine already paired. */
    suspend fun connect(machine: PairedMachine, identity: StaticKey): Session =
        withContext(Dispatchers.IO) {
            var lastFailure: Exception? = null
            for (address in machine.lan) {
                val socket = try {
                    connect(address)
                } catch (e: Exception) {
                    lastFailure = e
                    continue
                }
                // NOT `use`: the session owns the socket for as long as it
                // lives, and closing it here would end the session the moment
                // this function returned. But a handshake that *fails* owns
                // nothing, and leaving that socket open would leak one file
                // descriptor per refused connection — on a revoked phone that
                // keeps trying, which is exactly the phone this path is for.
                return@withContext try {
                    val session = Client.openSession(
                        input = socket.getInputStream(),
                        output = socket.getOutputStream(),
                        identity = identity,
                        desktopPublic = Device.checkKey(machine.desktopKey),
                    )
                    // The deadline comes OFF here, and only here: everything
                    // before this line ran against a peer that had not proved
                    // anything, and everything after it may sit idle for
                    // hours. A PTY with nobody typing produces no bytes, and a
                    // twenty-second read deadline on one would end somebody's
                    // terminal every twenty seconds of silence. The desktop
                    // does exactly the same thing at exactly the same point
                    // (`serve.rs`: "authenticated, so the handshake deadline
                    // comes off").
                    //
                    // The keepalive is what stands in for it: `apex-remoted`
                    // sends a `Ping` every fifteen seconds and `Session.receive`
                    // answers it, so a connection that has really gone away
                    // still fails on the next write rather than hanging for
                    // ever.
                    socket.soTimeout = 0
                    session
                } catch (e: Exception) {
                    runCatching { socket.close() }
                    throw e
                }
            }
            throw NoRouteToMachine("${machine.machine} did not answer on any known address", lastFailure)
        }

    private fun connect(address: String): Socket {
        val (host, port) = splitHostPort(address)
        val socket = Socket()
        socket.connect(InetSocketAddress(host, port), CONNECT_TIMEOUT_MS)
        // The desktop drops an unauthenticated peer after thirty seconds, so
        // this end sets a deadline of its own rather than waiting forever on a
        // machine that has already given up.
        //
        // It stays on after the handshake, which is right for everything this
        // app does today — the longest read is `measureRoundTrip`, which is a
        // frame away. P1-055 must CLEAR it before attaching a PTY: a terminal
        // may sit idle for hours, and a read deadline on one would end the
        // session every twenty seconds of silence.
        socket.soTimeout = HANDSHAKE_TIMEOUT_MS
        socket.tcpNoDelay = true
        return socket
    }

    /**
     * `host:port`, with an IPv6 literal in brackets.
     *
     * `substringAfterLast(':')` would take the last group of an unbracketed
     * IPv6 address and call it a port. The desktop writes them bracketed
     * (`net.rs` excludes link-local precisely because it cannot be written
     * down), so this reads brackets first and splits on the last colon only
     * when there are none.
     */
    internal fun splitHostPort(address: String): Pair<String, Int> {
        if (address.startsWith("[")) {
            val close = address.indexOf(']')
            require(close > 0) { "$address is an unclosed IPv6 literal" }
            val host = address.substring(1, close)
            val port = address.substring(close + 1).removePrefix(":")
            return host to (port.toIntOrNull() ?: DEFAULT_PORT)
        }
        val colon = address.lastIndexOf(':')
        if (colon < 0) return address to DEFAULT_PORT
        // More than one colon and no brackets: an IPv6 address that was written
        // down without them. There is no way to tell a port from a final group
        // here, and guessing produces the exact bug this function exists to
        // avoid — `2001:db8::1` becoming host `2001:db8:` on port 1, which
        // connects to nothing and reports a refusal that names the wrong thing.
        // So the whole string is the host and the default port is used.
        if (address.indexOf(':') != colon) return address to DEFAULT_PORT
        return address.substring(0, colon) to (address.substring(colon + 1).toIntOrNull() ?: DEFAULT_PORT)
    }

    private companion object {
        const val CONNECT_TIMEOUT_MS = 4_000
        const val HANDSHAKE_TIMEOUT_MS = 20_000

        /** `apex-remoted`'s listener. Only ever a fallback; the offer names the port. */
        const val DEFAULT_PORT = 7717
    }
}

/** Nothing the pairing code named could be reached. */
class NoRouteToMachine(message: String, cause: Throwable?) : Exception(message, cause)

/**
 * A box for a new device key, and whether it is really gated.
 *
 * The two travel together because the second is a property of the first and not
 * of the phone. "This device can authenticate" is a question asked of
 * `BiometricManager`; "the key protecting this pairing needs authentication" is
 * read off the key with `KeyInfo`, and they can differ — an alias created by an
 * older build, a screen lock removed and re-added. The desktop is told the
 * second, because that is the one that is true.
 */
class GatedBox(val box: SecretBox, val userVerification: Boolean)
