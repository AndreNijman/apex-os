package com.apexos.remote.core

import java.io.InputStream
import java.io.OutputStream

/**
 * The two conversations a device has with an APEX machine, over any pair of
 * streams.
 *
 * Streams and not a socket, deliberately. The LAN leg is a TCP connection and
 * the relay leg is a WebSocket carrying the same byte stream; `apex-remoted`
 * makes exactly this separation (`relay.rs`: "nothing above the transport
 * knows which leg it is on") and copying it here means the same code, and the
 * same tests, cover both. It is also what lets a test drive a full pairing
 * against a recorded transcript with no network at all.
 */
object Client {
    /**
     * Pair with the machine whose QR code produced [offer].
     *
     * Returns the desktop's answer, which arrives **inside** the finished
     * handshake whether it is a yes or a no — a plaintext refusal would tell
     * anybody watching that the machine is not currently pairing. A refusal is
     * therefore a [PairingException] raised after a handshake that completed,
     * not a connection that failed.
     */
    fun pair(
        input: InputStream,
        output: OutputStream,
        offer: PairingOffer,
        identity: StaticKey,
        deviceName: String,
        userVerification: Boolean,
        versions: List<Int> = SUPPORTED_REMOTE_PROTOCOL_VERSIONS,
        ephemerals: EphemeralSource = EphemeralSource.Secure,
    ): PairingAnswer {
        require(versions.isNotEmpty()) { "a client must speak at least one protocol revision" }
        // A WINDOW, not an equality test. The offer states the desktop's
        // revision in the clear, so pairing is the one moment the two ends
        // genuinely can agree, and refusing a desktop this build can talk to
        // purely because it is not the newest would be an invented failure.
        //
        // The handshake then runs at the DESKTOP's revision rather than this
        // build's preferred one, which is what makes the acceptance real: a
        // window that accepted the offer and then handshook at the wrong
        // version would fail a few lines later with a message about keys.
        if (offer.v !in versions) {
            throw PairingException(
                PairingError.Malformed(
                    "that machine speaks APEX Remote v${offer.v} and this app speaks " +
                        "${describeVersionWindow(versions)}. Update whichever is older: " +
                        "`sudo apex update` on the machine, or this app from its release page.",
                ),
            )
        }
        // The DESKTOP's revision, not `versions.first()`. This line is the
        // acceptance: a window that admitted the offer and then handshook at
        // this build's preferred revision would put a different number in the
        // Noise prologue, the handshake would fail, and `pair` would report
        // that the machine does not hold the key from the QR code — a message
        // about keys for a problem about versions. Measured: with
        // `versions.first()` the pairing test in `VersionWindowTest` does not
        // fail, it HANGS, because the responder dies and the initiator waits
        // for a reply that is never written.
        val version = offer.v
        // Refused here rather than by the desktop. Sending a name the desktop
        // will reject would burn the owner's pairing offer — `pairing::complete`
        // redeems the token before it validates the device — and cost them a
        // walk back to the machine for a new code.
        val name = Device.checkName(deviceName)
        val handshake = Noise.pairingInitiator(offer.desktopPublicKey(), version, ephemerals)
        val request = PairingRequest(
            key = identity.publicKeyText(),
            name = name,
            token = offer.token,
            userVerification = userVerification,
        )
        output.write(byteArrayOf(Transport.HELLO_PAIR))
        Transport.writeMessage(
            output,
            handshake.write(
                Pairing.json.encodeToString(PairingRequest.serializer(), request)
                    .toByteArray(Charsets.UTF_8),
            ),
        )
        val payload = try {
            handshake.read(Transport.readMessage(input))
        } catch (e: NoiseException) {
            // The handshake failing here is the QR code's whole purpose: the
            // machine on the other end does not hold the key that was on the
            // screen.
            throw PairingException(
                PairingError.BadDevice(
                    "that machine did not prove it holds the key from the QR code (${e.message})",
                ),
            )
        }
        val answer = try {
            Pairing.json.decodeFromString(PairingAnswer.serializer(), payload.toString(Charsets.UTF_8))
        } catch (e: Exception) {
            throw PairingException(PairingError.Malformed(e.message ?: "the answer did not parse"))
        }
        if (!answer.ok) {
            throw PairingException(PairingError.Refused(answer.error ?: PairingError.BadToken.message))
        }
        return answer
    }

    /**
     * Open a session with a machine this device is already paired with.
     *
     * Returns the machine's name and the encrypted channel. An unpaired or
     * revoked key gets **no second handshake message at all** — the desktop
     * closes the connection instead, because completing the handshake and then
     * refusing would confirm to a scanner that this machine's key is what they
     * think it is. So end-of-stream here is "not paired, or revoked", and the
     * two are deliberately not distinguishable.
     */
    fun openSession(
        input: InputStream,
        output: OutputStream,
        identity: StaticKey,
        desktopPublic: ByteArray,
        version: Int = REMOTE_PROTOCOL_VERSION,
        ephemerals: EphemeralSource = EphemeralSource.Secure,
    ): Session {
        val handshake = Noise.sessionInitiator(identity, desktopPublic, version, ephemerals)
        output.write(byteArrayOf(Transport.HELLO_SESSION))
        Transport.writeMessage(output, handshake.write(ByteArray(0)))
        val machine = try {
            handshake.read(Transport.readMessage(input)).toString(Charsets.UTF_8)
        } catch (e: java.io.EOFException) {
            throw SessionRefused(
                "this machine does not accept this device: it is not paired, or it has been revoked",
                e,
            )
        } catch (e: java.net.SocketException) {
            // The same refusal, arriving as a reset rather than as a clean
            // close. Which of the two a phone sees depends on the network in
            // between, not on what the desktop decided, so they must mean the
            // same thing here — otherwise a revoked device reports "not paired"
            // on Wi-Fi and "connection reset" on mobile data.
            throw SessionRefused(
                "this machine does not accept this device: it is not paired, or it has been revoked",
                e,
            )
        }
        return Session(machine, handshake.intoTransport(), input, output)
    }

    /**
     * Open a session, trying every protocol revision this build speaks.
     *
     * [openSession] is one attempt at one revision; this is the thing an app
     * should actually call. A desktop refuses a revision it does not speak by
     * closing the socket without replying, so each attempt needs its own
     * connection — hence [connect], which is called once per revision tried.
     *
     * Generic over the connection so that the caller keeps hold of whatever it
     * needs afterwards. The LAN leg needs its `Socket` back to clear the
     * handshake deadline, the relay leg needs its dialled link to close, and a
     * test needs neither; none of that belongs in here.
     *
     * Only a [SessionRefused] moves on to the next revision. Anything else —
     * a connection that could not be made, a desktop whose static key is not
     * the pinned one — is a different failure and is raised as one, because
     * retrying it at another revision would turn one honest error into a
     * handful of misleading ones.
     */
    fun <C> openSessionAcrossVersions(
        identity: StaticKey,
        desktopPublic: ByteArray,
        versions: List<Int> = SUPPORTED_REMOTE_PROTOCOL_VERSIONS,
        ephemerals: EphemeralSource = EphemeralSource.Secure,
        connect: () -> C,
        streams: (C) -> Pair<InputStream, OutputStream>,
        abandon: (C) -> Unit,
    ): Attached<C> {
        require(versions.isNotEmpty()) { "a client must speak at least one protocol revision" }
        var refused: SessionRefused? = null
        for (version in versions) {
            val connection = connect()
            val (input, output) = try {
                streams(connection)
            } catch (e: Throwable) {
                // A connection whose streams could not be taken is still a
                // connection, and dropping it here without closing it would
                // leak one file descriptor per revision tried.
                abandon(connection)
                throw e
            }
            try {
                val session = openSession(
                    input = input,
                    output = output,
                    identity = identity,
                    desktopPublic = desktopPublic,
                    version = version,
                    ephemerals = ephemerals,
                )
                return Attached(session, connection, version)
            } catch (e: SessionRefused) {
                // Keep the FIRST refusal as the cause. The window is tried
                // newest first, so the first is the attempt at the revision
                // this build would rather be speaking.
                if (refused == null) refused = e
                abandon(connection)
            } catch (e: Throwable) {
                // NOT another revision's turn. A key that is not the pinned
                // one, a stream that died mid-write, a parse that failed —
                // none of those get better at an older revision, and retrying
                // them would spend one dial per revision and then report a
                // protocol problem for something that was not one. Closed
                // first, because the throw leaves nobody else holding it.
                abandon(connection)
                throw e
            }
        }
        throw SessionRefused(exhaustedVersionWindow(versions), refused)
    }
}

/** A live session, the connection it is riding on, and the revision it agreed. */
class Attached<C>(val session: Session, val connection: C, val version: Int)

/** The desktop hung up during the session handshake. */
class SessionRefused(message: String, cause: Throwable? = null) : Exception(message, cause)

/**
 * An open session: the machine's name, and frames over an encrypted channel.
 *
 * Not thread-safe for writing, and that is a property of the protocol rather
 * than an omission. The Noise nonce is a message counter and neither end
 * chooses it, so two threads sealing concurrently would produce two messages
 * claiming the same one — `apex-remoted` funnels everything it sends through a
 * single `Sealer` for exactly this reason, and a client must do the same.
 */
class Session internal constructor(
    override val machine: String,
    private val channel: NoiseChannel,
    private val input: InputStream,
    private val output: OutputStream,
) : FrameChannel {
    private val writeLock = Any()
    private val outstanding = java.util.concurrent.ConcurrentHashMap<Long, Long>()
    private val nextToken = java.util.concurrent.atomic.AtomicLong(0)

    /**
     * The last measured round trip, in milliseconds, or `null` before the first
     * [ping] comes back.
     *
     * P1-052 asks for the connection path and its quality to be visible at
     * *both* ends. This is this end's half, and it is measured on a frame that
     * crosses the same path as everything else — a ping to the relay's front
     * door would report the health of a machine nobody is talking to.
     */
    @Volatile
    var roundTripMs: Long? = null
        private set

    /** Send one frame. Safe to call from several threads; serialised here. */
    override fun send(frame: Frame) {
        val plaintext = frame.encode()
        synchronized(writeLock) {
            Transport.writeMessage(output, channel.seal(plaintext))
        }
    }

    /** Send a run of terminal bytes, split into as many frames as it needs. */
    override fun sendData(channelId: UInt, bytes: ByteArray) {
        for (frame in Frame.dataFrames(channelId, bytes)) send(frame)
    }

    /** Measure the connection. The answer lands in [roundTripMs]. */
    fun ping() {
        val token = nextToken.incrementAndGet()
        outstanding[token] = System.nanoTime()
        send(Frame.Ping(token))
    }

    /**
     * Read one frame, answering keepalives on the way.
     *
     * `apex-remoted` sends a `Ping` every fifteen seconds — it is the only
     * traffic an idle session has, and an idle session with no traffic is one a
     * NAT eventually forgets — and it measures the round trip from the `Pong`
     * that comes back. A client that handed those frames to its caller would
     * make every caller handle them, and a client that ignored them would leave
     * the desktop reporting an unknown connection quality forever. So they are
     * answered here and never surface: exactly what the desktop's own frame
     * loop does with the mirror image of this.
     *
     * One reader only. The receiving nonce is a counter too, and two readers
     * would each advance it past the other's message.
     */
    override fun receive(): Frame {
        while (true) {
            val frame = readFrame()
            when (frame) {
                is Frame.Ping -> send(Frame.Pong(frame.token))
                is Frame.Pong -> {
                    // A token this end never sent is ignored rather than timed.
                    // The desktop applies the same rule in the other direction
                    // and for the same reason: a peer that echoed a number of
                    // its own choosing could otherwise report any quality it
                    // liked, including a good one for a connection that is
                    // unusable.
                    val sent = outstanding.remove(frame.token)
                    if (sent != null) {
                        roundTripMs = (System.nanoTime() - sent) / 1_000_000
                    }
                }
                else -> return frame
            }
        }
    }

    /**
     * When a frame last arrived, keepalives included.
     *
     * Written here rather than in [receive] precisely because [receive] hides
     * the pings, and the pings are the only traffic an idle terminal has. See
     * [FrameChannel.lastFrameNanos].
     */
    @Volatile
    override var lastFrameNanos: Long? = System.nanoTime()
        private set

    /** One frame off the wire, keepalives and all. The only reader of [input]. */
    private fun readFrame(): Frame = Frame.decode(channel.open(Transport.readMessage(input)))
        .also { lastFrameNanos = System.nanoTime() }

    /**
     * Ping, and read until the answer comes back. Returns the round trip in
     * milliseconds.
     *
     * Separate from [ping] because the two have different callers. [ping] is
     * for a session with a frame loop already running: it posts the ping and
     * the loop times the answer whenever it arrives. This is for a session that
     * has nothing else to do yet — the moment after a connection opens, when
     * the only question is whether frames actually cross this path, which a
     * completed handshake does not answer.
     *
     * **Only before a channel is open.** Anything that is not a keepalive
     * belongs to a caller, and this method has nowhere to put it, so it refuses
     * rather than swallowing somebody's terminal. A silent peer is the
     * transport's problem: whatever read deadline the stream carries is what
     * ends the wait.
     */
    fun measureRoundTrip(): Long {
        val token = nextToken.incrementAndGet()
        val sentAt = System.nanoTime()
        outstanding[token] = sentAt
        send(Frame.Ping(token))
        while (true) {
            when (val frame = readFrame()) {
                is Frame.Ping -> send(Frame.Pong(frame.token))
                is Frame.Pong -> {
                    // Somebody else's token, or one already answered. Ignored
                    // for the same reason [receive] ignores it: a peer that
                    // echoed a number of its own choosing must not be able to
                    // report a quality it did not earn.
                    if (frame.token == token) {
                        outstanding.remove(token)
                        return ((System.nanoTime() - sentAt) / 1_000_000).also { roundTripMs = it }
                    }
                }
                else -> throw WireException(
                    WireError.Malformed(
                        "a $frame arrived while measuring the connection; measure before opening a channel",
                    ),
                )
            }
        }
    }

    /**
     * Hang up.
     *
     * [Closeable] and not a bare method, so a caller can `use` a session and a
     * screen that goes away cannot leave a socket open. Both streams are
     * closed and failures are swallowed: the far end may already be gone, and
     * a throw from a close is a throw that hides whatever ended the session.
     *
     * Closing does not invalidate the channel's keys, and nothing here tries to
     * pretend otherwise — a Noise transport has no "closed" state. What it does
     * is end the stream, after which [send] and [receive] fail. That is the
     * only signal a caller needs.
     */
    override fun close() {
        runCatching { output.close() }
        runCatching { input.close() }
    }
}

/**
 * The protocol revision this build speaks.
 *
 * Exchanged in the pairing payload and again in the handshake prologue, so a
 * device and a desktop that disagree find out before either has sent a frame
 * the other would misread. Separate from `apex-agentd`'s own protocol version,
 * which versions what travels *inside* a control frame.
 */
const val REMOTE_PROTOCOL_VERSION: Int = 1

/**
 * Every protocol revision this build can speak, **newest first**.
 *
 * The phone and the desktop are updated by different people at different
 * times — the OS through `apex update`, the app through its own release — so
 * they are routinely at different ages, and a client that speaks exactly one
 * revision stops working on the day the OS moves. This list is what makes that
 * drift survivable instead of fatal.
 *
 * It can be a plain list, with no negotiation and no round trip, because of
 * how the version is carried. It is hashed into the Noise prologue and never
 * transmitted (`noise.rs`: "Noise binds the prologue into the handshake hash,
 * so two ends that disagree about either simply fail to complete"), so a
 * client cannot ASK which revision a desktop speaks — but it can try one, and
 * a wrong guess costs a single connection and leaks nothing. [Client] tries
 * each in turn.
 *
 * Newest first so that an up-to-date pair connects on the first attempt and
 * only a genuinely old machine pays for a second.
 *
 * **Adding an entry is a promise**: this build must be able to hold a whole
 * session at that revision, not merely complete its handshake. Removing the
 * oldest entry is what drops support for desktops that have not updated, and
 * it should be done deliberately and written in the release notes, because to
 * the owner of such a desktop it is indistinguishable from the app breaking.
 *
 * Written as plain integers rather than as `listOf(REMOTE_PROTOCOL_VERSION)`
 * because `android/tools/release-artifacts.sh` READS THIS LINE to put the
 * window into the release metadata, and from there onto the Releases page. A
 * page that claims "an app one revision behind still connects" without being
 * able to state the window is a compatibility promise nobody can check — the
 * defect class this unit exists to close. The literal cannot drift from the
 * constant: `VersionWindowTest` asserts that this list's maximum IS
 * [REMOTE_PROTOCOL_VERSION] and that the list is newest-first and has no
 * duplicates.
 */
val SUPPORTED_REMOTE_PROTOCOL_VERSIONS: List<Int> = listOf(1)

/** "v1", or "v1 to v3" — the window as a person would say it. */
internal fun describeVersionWindow(versions: List<Int>): String {
    val lo = versions.min()
    val hi = versions.max()
    return if (lo == hi) "v$lo" else "v$lo to v$hi"
}

/**
 * What the app says when every revision it speaks was refused.
 *
 * It names BOTH causes, because the phone genuinely cannot tell them apart and
 * pretending otherwise is what made this failure so bad before: the desktop
 * drops the socket without a reply, so a protocol mismatch and a revoked
 * device are byte-for-byte identical from here. The old message asserted the
 * revocation and never mentioned the version, which meant that bumping the
 * protocol told every installed phone it had been thrown out.
 *
 * It also says how to find out, rather than leaving the user to guess: `apex
 * remote status` prints the desktop's protocol version, and this sentence
 * prints the app's.
 */
internal fun exhaustedVersionWindow(versions: List<Int>): String =
    "this machine did not accept this device. Either the device was unpaired or revoked " +
        "on it, or the machine and this app no longer speak the same APEX Remote protocol " +
        "— this app speaks ${describeVersionWindow(versions)}. Run `apex remote status` on " +
        "the machine to see the version it speaks, and update whichever side is older."
