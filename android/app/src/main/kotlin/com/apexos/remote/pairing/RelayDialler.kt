package com.apexos.remote.pairing

import com.apexos.remote.core.Opening
import com.apexos.remote.core.Relay
import com.apexos.remote.core.RelayEndpoint
import com.apexos.remote.core.RelayError
import com.apexos.remote.core.RelayException
import com.apexos.remote.core.RelayLink
import com.apexos.remote.core.Rendezvous
import java.io.InputStream
import java.io.OutputStream
import java.net.InetSocketAddress
import java.net.Socket
import javax.net.ssl.SSLSocket
import javax.net.ssl.SSLSocketFactory

/**
 * Reaching a relay: the socket, the TLS, and nothing above them.
 *
 * The other half of the split `relay.rs` makes. `:core`'s [Relay] is the codec
 * and is generic over streams; this is the part that has an address and a
 * platform, exactly as `apex-remoted`'s own `relay::dial` is. Keeping them
 * apart is what makes TLS cost nothing in the codec — and it is why every byte
 * test in `RelayTest` runs without a socket.
 *
 * A dialled relay is a [RelayLink], whose [RelayLink.input] and
 * [RelayLink.output] are the same pair a [Socket] gives [PairingService] on the
 * LAN leg. That is the whole seam: the caller cannot tell which it is holding.
 */
object RelayDialler {
    /** As long as a phone should wait for a relay before trying something else. */
    const val CONNECT_TIMEOUT_MS = 6_000

    /**
     * Longer than the LAN leg's, and deliberately.
     *
     * A relay adds a round trip to Cloudflare and back to a desktop that may be
     * on the other side of the world, and the desktop still drops an
     * unauthenticated peer after thirty seconds. Twenty-five leaves the phone
     * saying the relay did not answer slightly before the far end gives up.
     */
    const val HANDSHAKE_TIMEOUT_MS = 25_000

    /**
     * Dial [endpoint] as the guest at [rendezvous] and complete the upgrade.
     *
     * The returned link owns the socket and closing it closes the socket. A
     * dial that FAILS owns nothing and closes it here, because a phone that has
     * been revoked keeps trying and one leaked descriptor per attempt is a
     * phone that eventually cannot open anything.
     */
    fun dial(
        endpoint: RelayEndpoint,
        rendezvous: String,
        role: Rendezvous.Role = Rendezvous.Role.GUEST,
        connectTimeoutMs: Int = CONNECT_TIMEOUT_MS,
        handshakeTimeoutMs: Int = HANDSHAKE_TIMEOUT_MS,
    ): Dialled {
        val plain = Socket()
        plain.connect(InetSocketAddress(endpoint.host, endpoint.port), connectTimeoutMs)
        val socket: Socket = try {
            plain.soTimeout = handshakeTimeoutMs
            // Nagle off: the carried stream is somebody's terminal, and
            // coalescing a keystroke with whatever comes next is exactly the
            // latency this feature is judged on.
            plain.tcpNoDelay = true
            if (endpoint.secure) secure(plain, endpoint) else plain
        } catch (e: Throwable) {
            runCatching { plain.close() }
            throw e
        }

        return try {
            val input: InputStream = socket.getInputStream()
            val output: OutputStream = socket.getOutputStream()
            val opening = Opening.fresh()
            // Written INTO the TLS session, never before it. The ordering is
            // the point: the rendezvous id lives in this request's path, and a
            // client that wrote it in the clear would publish to every on-path
            // observer the one value a device derives from the key it pinned.
            output.write(opening.request(endpoint, rendezvous, role))
            output.flush()
            opening.accept(input)
            // The deadline comes off HERE and only here, at the same point the
            // LAN leg drops its own: everything before this line ran against a
            // relay that had proved nothing, and everything after it may sit
            // idle for hours while nobody types. The desktop's fifteen-second
            // keepalive is what stands in for it.
            socket.soTimeout = 0
            Dialled(
                socket = socket,
                link = RelayLink(input, output) { runCatching { socket.close() } },
            )
        } catch (e: Throwable) {
            runCatching { socket.close() }
            throw e
        }
    }

    /**
     * TLS, with the relay's name actually checked.
     *
     * **`SSLSocket` does not verify the hostname by default.** It validates the
     * certificate chain and then accepts a valid certificate for *any* name, so
     * without the line below this would trust whoever answered the address —
     * and the rendezvous id in the request path would go to them. That failure
     * passes every test that only asks whether TLS was negotiated, which is
     * why it is asserted directly in `RelayDiallerTest`.
     *
     * Layered over the connected socket rather than dialled fresh so the name
     * in the URL is what SNI carries and what the certificate is checked
     * against — never an address that was resolved on the way here. Cloudflare
     * requires SNI, so a relay dialled by address answers for the wrong site or
     * not at all.
     */
    private fun secure(plain: Socket, endpoint: RelayEndpoint): SSLSocket {
        val tls = configureTls(plain, endpoint, SSLSocketFactory.getDefault() as SSLSocketFactory)
        tls.startHandshake()
        return tls
    }

    /**
     * Everything about the TLS socket except the handshake itself.
     *
     * Split out so `RelayDiallerTest` can assert the identification algorithm
     * on a real [SSLSocket] without needing a certificate authority and a
     * server to misuse one. The line it is asserting is the difference between
     * checking the relay's name and trusting whoever answered the address, and
     * it is invisible to any test that only asks whether TLS was negotiated.
     */
    internal fun configureTls(
        plain: Socket,
        endpoint: RelayEndpoint,
        factory: SSLSocketFactory,
    ): SSLSocket {
        val tls = factory.createSocket(plain, endpoint.host, endpoint.port, true) as SSLSocket
        val params = tls.sslParameters
        params.endpointIdentificationAlgorithm = "HTTPS"
        tls.sslParameters = params
        // Explicit rather than implied by the factory: a relay reached over an
        // obsolete protocol version is a relay reached in something close to
        // the clear, and the deployment is a Cloudflare Worker that has spoken
        // 1.2 and 1.3 for years.
        tls.enabledProtocols = tls.supportedProtocols.filter { it == "TLSv1.3" || it == "TLSv1.2" }
            .ifEmpty {
                throw RelayException(
                    RelayError.Upgrade("this device offers no TLS version a relay will accept"),
                )
            }
            .toTypedArray()
        return tls
    }

    /**
     * A relay connection and the socket underneath it.
     *
     * The socket is exposed for the same reason the LAN leg keeps one: a
     * caller may need to close it out of band. Nothing reads the *protocol*
     * through it — the moment something above the transport asked this object
     * which leg it was on, the design would be gone.
     */
    class Dialled(val socket: Socket, val link: RelayLink)
}
