package com.apexos.remote.pairing

import com.apexos.remote.core.Client
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
        boxFor: suspend (deviceId: String) -> SecretBox,
        userVerification: Boolean,
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
        val answer = handshake(offer, identity, deviceName, userVerification)
        return MachineStore.record(identity, offer, answer, boxFor(identity.deviceId()), nowMs)
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
                // this function returned.
                return@withContext Client.openSession(
                    input = socket.getInputStream(),
                    output = socket.getOutputStream(),
                    identity = identity,
                    desktopPublic = machine.let { com.apexos.remote.core.Device.checkKey(it.desktopKey) },
                )
            }
            throw NoRouteToMachine("${machine.machine} did not answer on any known address", lastFailure)
        }

    private fun connect(address: String): Socket {
        val (host, port) = splitHostPort(address)
        val socket = Socket()
        socket.connect(InetSocketAddress(host, port), CONNECT_TIMEOUT_MS)
        // The desktop drops an unauthenticated peer after thirty seconds. This
        // end sets its own deadline for the handshake and takes it off once a
        // session is open, because a PTY may sit idle for hours and a read
        // deadline on one would end the terminal.
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
