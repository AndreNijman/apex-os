package com.rimeos.remote.pairing

import com.rimeos.remote.core.Client
import com.rimeos.remote.core.Device
import com.rimeos.remote.core.InMemoryStaticKey
import com.rimeos.remote.core.MachineStore
import com.rimeos.remote.core.PairedMachine
import com.rimeos.remote.core.PairingAnswer
import com.rimeos.remote.core.PairingOffer
import com.rimeos.remote.core.SecretBox
import com.rimeos.remote.core.RelayEndpoint
import com.rimeos.remote.core.Rendezvous
import com.rimeos.remote.core.Session
import com.rimeos.remote.core.StaticKey
import com.rimeos.remote.core.link.Cancel
import com.rimeos.remote.core.link.ConnectPlan
import com.rimeos.remote.core.link.PathRace
import com.rimeos.remote.core.link.RaceLost
import com.rimeos.remote.core.link.RelayRetry
import com.rimeos.remote.core.link.Route
import android.util.Log
import java.net.InetSocketAddress
import java.net.Socket
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * Pairing and connecting, over a real socket, off the main thread.
 *
 * `:core` is generic over streams because the LAN leg is TCP and the relay leg
 * is a WebSocket carrying the same bytes; this is where both are chosen
 * between. Everything here runs on [Dispatchers.IO] — a socket touched from the
 * main thread is an immediate `NetworkOnMainThreadException`, and it is a crash
 * that never happens on a laptop running the unit tests.
 *
 * **Pairing is LAN first, always**: the addresses in the offer, in order, and
 * the relay only when none answered. A pairing happens once, at the machine,
 * which is on the same network as the phone in practice.
 *
 * **Connecting races**, and that is a reversal of what this note used to say
 * ("a client that raced both paths … would leak a rendezvous connection every
 * time"). The sequential walk it defended cost four seconds per stored address
 * on a network that drops SYNs, before the relay was even asked — most of why
 * connecting took ages. `ConnectPlan` gives the LAN a 300 ms head start, so on
 * the machine's own network the relay is normally never dialled; when it is,
 * it learns that this phone connected at that moment and nothing else, since
 * the rendezvous is a hash of the machine's key and everything after the
 * upgrade is Noise ciphertext. The trade is written down in
 * docs/remote-live-contract.md §3 and in `ConnectPlan`.
 *
 * Below [Client.pair] and [Client.openSession] the two legs are the same thing:
 * an [java.io.InputStream] and an [java.io.OutputStream]. Which one was used is
 * returned to the caller as a [Rendezvous.Path] so the user can be told — it is
 * never read by anything that speaks the protocol.
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
        // `rime remote revoke`. Cancelling here costs nothing at all.
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
        val relay = offer.relay
            ?: throw NoRouteToMachine(
                "none of the addresses in the pairing code answered: ${offer.lan.joinToString()}" +
                    " and it names no relay",
                lastFailure,
            )
        // The relay leg. The rendezvous is derived from the key in the QR code,
        // so nothing has to be exchanged for the two ends to meet — and the
        // relay operator sees a hash rather than the desktop's public key.
        val dialled = try {
            RelayDialler.dial(RelayEndpoint.parse(relay), Rendezvous.idFor(offer.desktopPublicKey()))
        } catch (e: Exception) {
            throw NoRouteToMachine(
                "neither ${offer.lan.joinToString().ifEmpty { "any local address" }} nor the " +
                    "relay answered",
                e,
            )
        }
        dialled.link.use {
            return@withContext Client.pair(
                input = it.input,
                output = it.output,
                offer = offer,
                identity = identity,
                deviceName = deviceName,
                userVerification = userVerification,
            )
        }
    }

    /** Open a session with a machine already paired. */
    suspend fun connect(machine: PairedMachine, identity: StaticKey): Session =
        open(machine, identity).session

    /**
     * Open a session, and say which way it went.
     *
     * The path is here and not on [Session] deliberately. `docs/remote.md`
     * requires a relay path to disclose what the relay can and cannot see, so
     * somebody has to be told; but putting it on the session would hand it to
     * every caller that speaks the protocol, and the moment one of them
     * branched on it the relay leg would stop being indistinguishable from the
     * LAN one. It is returned to whoever chose the path, which is the only
     * layer entitled to know.
     *
     * ## Every path at once
     *
     * The stored LAN addresses and the relay are raced ([PathRace]) on the
     * schedule [ConnectPlan] draws up from [lastGood] — the path that worked
     * last time goes first — and the first completed Noise handshake wins.
     * The losers' sockets are closed mid-dial. One line goes to the log per
     * connection, `connected to <machine> via <lan|relay> in <N> ms`, because
     * "it takes ages to connect" is a claim, and that line is how it is
     * measured on the phone itself.
     *
     * When every path fails, a path that REACHED the machine and was refused
     * is the error raised — a revoked device, a protocol the machine does not
     * speak — and "unreachable" only when nothing answered at all.
     */
    suspend fun open(
        machine: PairedMachine,
        identity: StaticKey,
        lastGood: Route? = Route.parse(machine.lastRoute),
    ): Connected = withContext(Dispatchers.IO) {
        val desktop = Device.checkKey(machine.desktopKey)
        val plan = ConnectPlan.schedule(machine.lan, machine.relay != null, lastGood)
        if (plan.isEmpty()) {
            throw NoRouteToMachine("${machine.machine} has no address and no relay to try", null)
        }
        val race = PathRace<Connected>(
            dial = { route, cancel ->
                when (route) {
                    is Route.Lan -> dialLan(route, desktop, identity, cancel)
                    Route.Relay -> dialRelay(machine, desktop, identity, cancel)
                }
            },
            abandon = { it.hangUp() },
        )
        val won = try {
            race.run(plan)
        } catch (lost: RaceLost) {
            lost.answer { it !is Unreached }?.let { throw it }
            val last = lost.failures.lastOrNull()?.second
            throw NoRouteToMachine(
                "${machine.machine} did not answer on any known address" +
                    (if (machine.relay != null) " or through the relay" else "") +
                    (last?.message?.let { ": $it" } ?: ""),
                (last as? Unreached)?.cause ?: last,
            )
        }
        Log.i(TAG, "connected to ${machine.machine} via ${won.route.kind} in ${won.elapsedMs} ms")
        won.result.also { it.route = won.route; it.elapsedMs = won.elapsedMs }
    }

    /** One LAN address, as a racer. */
    private fun dialLan(
        route: Route.Lan,
        desktop: ByteArray,
        identity: StaticKey,
        cancel: Cancel,
    ): Connected {
        // `reached` separates "this address did not answer" from "this
        // address answered and the conversation failed". The first is
        // [Unreached]; the second is a real answer and must be raised as
        // itself, or a machine that refused this device would be reported as
        // unreachable.
        //
        // It is needed because the connection is made INSIDE
        // `openSessionAcrossVersions` — a desktop refuses a protocol revision
        // by closing the socket, so each revision tried needs its own
        // connection.
        var reached = false
        // NOT `use`: the session owns the socket for as long as it lives. But
        // a handshake that FAILS owns nothing, and leaving that socket open
        // would leak one file descriptor per refused connection. `abandon`
        // closes those.
        val attached = try {
            Client.openSessionAcrossVersions(
                identity = identity,
                desktopPublic = desktop,
                connect = { connect(route.address, cancel).also { reached = true } },
                streams = { it.getInputStream() to it.getOutputStream() },
                abandon = { runCatching { it.close() } },
            )
        } catch (e: Exception) {
            if (!reached) throw Unreached(e)
            throw e
        }
        val socket = attached.connection
        return try {
            // The deadline comes OFF here, and only here: everything before
            // this line ran against a peer that had not proved anything, and
            // everything after it may sit idle for hours. A PTY with nobody
            // typing produces no bytes, and a twenty-second read deadline on
            // one would end somebody's terminal every twenty seconds of
            // silence. The desktop does exactly the same thing at exactly the
            // same point (`serve.rs`: "authenticated, so the handshake
            // deadline comes off").
            //
            // The keepalive is what stands in for it: `rime-remoted` sends a
            // `Ping` every fifteen seconds and `Session.receive` answers it,
            // and the control link's watchdog treats forty seconds without one
            // as a dead connection.
            socket.soTimeout = 0
            Connected(attached.session, Rendezvous.Path.LAN) { runCatching { socket.close() } }
        } catch (e: Exception) {
            runCatching { socket.close() }
            throw e
        }
    }

    /**
     * The relay, as a racer.
     *
     * Off the machine's network. The rendezvous is derived from the key
     * pinned when this phone paired, so a relay that wanted to stand in the
     * middle would still have to complete a Noise handshake against a key it
     * does not hold.
     *
     * A 409 — no desktop waiting at the rendezvous right now — is retried on
     * [RelayRetry]'s schedule: the desktop keeps one socket waiting and a
     * second guest a moment after the first finds it used up. Dialled once
     * per protocol revision tried, like the LAN leg, which is the other half
     * of why `SUPPORTED_REMOTE_PROTOCOL_VERSIONS` is newest first.
     */
    private fun dialRelay(
        machine: PairedMachine,
        desktop: ByteArray,
        identity: StaticKey,
        cancel: Cancel,
    ): Connected {
        val endpoint = RelayEndpoint.parse(requireNotNull(machine.relay))
        val rendezvous = machine.rendezvousId()
        var reached = false
        val attached = try {
            Client.openSessionAcrossVersions(
                identity = identity,
                desktopPublic = desktop,
                connect = {
                    RelayRetry.onNoDesktop(cancel) {
                        RelayDialler.dial(
                            endpoint,
                            rendezvous,
                            onSocket = { s -> cancel.onCancel { runCatching { s.close() } } },
                        )
                    }.also { reached = true }
                },
                streams = { it.link.input to it.link.output },
                abandon = { runCatching { it.link.close() } },
            )
        } catch (e: Exception) {
            if (!reached) throw Unreached(e)
            throw e
        }
        val dialled = attached.connection
        return try {
            // Here, and for the same reason as the LAN leg: the handshake ran
            // under a deadline because the far end had proved nothing, and a
            // terminal nobody is typing at produces no bytes for hours.
            dialled.socket.soTimeout = 0
            Connected(attached.session, Rendezvous.Path.RELAY) { runCatching { dialled.link.close() } }
        } catch (e: Exception) {
            runCatching { dialled.link.close() }
            throw e
        }
    }

    private fun connect(address: String, cancel: Cancel? = null): Socket {
        val (host, port) = splitHostPort(address)
        val socket = Socket()
        // Before the connect, so a race won elsewhere can end a SYN that a
        // firewall is silently dropping instead of waiting out its timeout.
        cancel?.onCancel { runCatching { socket.close() } }
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
        /** The log tag, stable so `adb logcat -s RimeRemote` finds every connection. */
        const val TAG = "RimeRemote"

        const val CONNECT_TIMEOUT_MS = 4_000
        const val HANDSHAKE_TIMEOUT_MS = 20_000

        /** `rime-remoted`'s listener. Only ever a fallback; the offer names the port. */
        const val DEFAULT_PORT = 7717
    }
}

/**
 * A live session and the path it came in on.
 *
 * Two values rather than one because the second is not the session's business:
 * see [PairingService.open]. [route] and [elapsedMs] are filled in by the race
 * that produced it — which address, or the relay, and how long the connection
 * took — for the store's last-good path and for the screen.
 */
class Connected(
    val session: Session,
    val path: Rendezvous.Path,
    /** Closes the socket under [session], for a racer that finished second. */
    internal val hangUp: () -> Unit = { runCatching { session.close() } },
) {
    var route: Route? = null
        internal set
    var elapsedMs: Long = 0
        internal set
}

/**
 * A path that did not reach the machine at all: nothing answered.
 *
 * A marker for the race, which prefers any failure that is NOT this one when
 * everything has failed — see [PairingService.open].
 */
internal class Unreached(cause: Throwable) : Exception(cause.message, cause)

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
