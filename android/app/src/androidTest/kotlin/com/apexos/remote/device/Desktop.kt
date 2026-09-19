package com.apexos.remote.device

import androidx.test.platform.app.InstrumentationRegistry
import java.io.BufferedReader
import java.net.InetSocketAddress
import java.net.Socket
import org.json.JSONObject

/**
 * The desktop, as the on-device suite can reach it.
 *
 * `apex-remoted`'s control socket is a unix socket on the computer and a phone
 * can never touch one, so `tools/run-device-suite.sh` puts a line-JSON broker
 * in front of it and passes this test the address. Every offer the suite pairs
 * with, and every revocation it asserts against, is minted by the REAL daemon
 * through that broker — the broker itself decides nothing.
 *
 * A missing argument is a **failure and never a skip**. An instrumented suite
 * that quietly passed when nobody told it where the desktop was would be a
 * green run asserting nothing, which is the failure mode this whole program
 * has shipped more than once.
 */
object Desktop {

    /** An instrumentation argument, or a failure that names it. */
    fun arg(name: String): String {
        val value = InstrumentationRegistry.getArguments().getString(name)
        check(!value.isNullOrBlank()) {
            "this suite needs -e $name. It is an END-TO-END suite against a real " +
                "apex-remoted, so without it there is nothing to test and a pass would " +
                "mean nothing. Run it through android/tools/run-device-suite.sh."
        }
        return value
    }

    /** `host:port` of the daemon's LAN listener, as the runner measured it. */
    fun lanAddress(): String = arg("lan")

    /** The broker: `host:port`. */
    private fun broker(): Pair<String, Int> {
        val a = arg("broker")
        return a.substringBeforeLast(':') to a.substringAfterLast(':').toInt()
    }

    /**
     * One request on the broker; the reply is one JSON line.
     *
     * `org.json`, which Android ships, rather than `kotlinx.serialization`:
     * these replies are the DESKTOP's shapes and this side should not need a
     * model of them to read one field. A test harness that had to be kept in
     * step with `serde` would be one more thing that can disagree.
     */
    fun ask(request: String): JSONObject {
        val (host, port) = broker()
        Socket().use { socket ->
            socket.connect(InetSocketAddress(host, port), 8_000)
            socket.soTimeout = 30_000
            socket.getOutputStream().write((request + "\n").toByteArray())
            socket.getOutputStream().flush()
            val line = socket.getInputStream().bufferedReader().let(BufferedReader::readLine)
                ?: error("the broker closed without answering $request")
            return JSONObject(line)
        }
    }

    /** A fresh pairing offer, minted by the real daemon. */
    fun offerPayload(): String {
        val reply = ask("""{"cmd":"pair"}""")
        check(reply.has("qr")) { "apex-remoted did not mint an offer: $reply" }
        return reply.getString("qr")
    }

    /** Every device the desktop knows, by id. */
    fun deviceIds(): List<String> = devices().map { it.getString("id") }

    private fun devices(): List<JSONObject> {
        val reply = ask("""{"cmd":"devices"}""")
        val array = reply.optJSONArray("devices") ?: return emptyList()
        return (0 until array.length()).map { array.getJSONObject(it) }
    }

    /** The desktop's view of one device, or null. */
    fun device(id: String): JSONObject? = devices().firstOrNull { it.optString("id") == id }

    /** Take a device's access away, as `apex remote revoke` does. */
    fun revoke(id: String): JSONObject = ask("""{"cmd":"revoke","device":"$id"}""")
}
