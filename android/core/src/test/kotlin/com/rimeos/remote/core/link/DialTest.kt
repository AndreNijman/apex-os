package com.rimeos.remote.core.link

import com.rimeos.remote.core.RelayError
import com.rimeos.remote.core.RelayException
import java.io.IOException
import java.util.concurrent.ConcurrentLinkedQueue
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertSame
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.assertThrows

/**
 * Which paths a connection tries, in what order, and which one it keeps.
 *
 * The schedule is pure and asserted value by value. The race runs real
 * threads against fake dials, and is driven by latches rather than sleeps
 * wherever an ordering is the thing under test — a race test that passes
 * because the machine happened to be idle is the flake this file exists not
 * to be.
 */
class DialTest {
    private val home = "192.168.1.232:7717"
    private val dock = "10.0.0.5:7717"

    // ---- the schedule ----------------------------------------------------

    @Test
    fun `with nothing remembered, every LAN address goes at once and the relay waits its head start`() {
        val plan = ConnectPlan.schedule(listOf(home, dock), hasRelay = true, lastGood = null)
        assertEquals(
            listOf(
                Scheduled(Route.Lan(home), 0),
                Scheduled(Route.Lan(dock), 0),
                Scheduled(Route.Relay, ConnectPlan.RELAY_HEAD_START_MS),
            ),
            plan,
        )
        assertEquals(300, ConnectPlan.RELAY_HEAD_START_MS, "the contract's head start")
    }

    @Test
    fun `the path that worked last time goes first`() {
        val lan = ConnectPlan.schedule(listOf(home, dock), hasRelay = true, lastGood = Route.Lan(dock))
        assertEquals(Scheduled(Route.Lan(dock), 0), lan.first())
        // The others follow it closely rather than queueing behind it.
        assertEquals(Scheduled(Route.Lan(home), ConnectPlan.LAN_STAGGER_MS), lan[1])
        assertEquals(Scheduled(Route.Relay, ConnectPlan.RELAY_HEAD_START_MS), lan[2])

        // The relay last time: it goes at once, and so does the LAN — at home
        // again, the LAN still wins on speed.
        val relay = ConnectPlan.schedule(listOf(home), hasRelay = true, lastGood = Route.Relay)
        assertEquals(
            listOf(Scheduled(Route.Relay, 0), Scheduled(Route.Lan(home), 0)),
            relay,
        )
    }

    @Test
    fun `a remembered path that is no longer offered is not tried`() {
        val plan = ConnectPlan.schedule(listOf(home), hasRelay = false, lastGood = Route.Lan(dock))
        assertEquals(listOf(Scheduled(Route.Lan(home), 0)), plan)
        assertEquals(
            listOf(Scheduled(Route.Lan(home), 0)),
            ConnectPlan.schedule(listOf(home), hasRelay = false, lastGood = Route.Relay),
        )
    }

    @Test
    fun `with no LAN address the relay does not wait for anything`() {
        assertEquals(
            listOf(Scheduled(Route.Relay, 0)),
            ConnectPlan.schedule(emptyList(), hasRelay = true, lastGood = null),
        )
        assertTrue(ConnectPlan.schedule(emptyList(), hasRelay = false, lastGood = null).isEmpty())
    }

    @Test
    fun `a route survives the store as a string`() {
        for (r in listOf(Route.Lan(home), Route.Lan("[fd00::1]:7717"), Route.Relay)) {
            assertEquals(r, Route.parse(r.key))
        }
        assertNull(Route.parse(null))
        assertNull(Route.parse("carrier-pigeon"))
        assertNull(Route.parse("lan:"))
        assertEquals("lan", Route.Lan(home).kind)
        assertEquals("relay", Route.Relay.kind)
    }

    // ---- the race ----------------------------------------------------------

    private class Conn(val route: Route) {
        @Volatile
        var closed = false
    }

    @Test
    fun `the first path to finish wins, and a later finisher is closed rather than kept`() {
        val slowMayFinish = CountDownLatch(1)
        val relayDialling = CountDownLatch(1)
        val abandoned = ConcurrentLinkedQueue<Conn>()
        val race = PathRace<Conn>(
            dial = { route, _ ->
                if (route == Route.Relay) {
                    relayDialling.countDown()
                    slowMayFinish.await(5, TimeUnit.SECONDS)
                } else {
                    // Not before the relay is actually dialling, or the relay
                    // would find the race over and never dial at all.
                    relayDialling.await(5, TimeUnit.SECONDS)
                }
                Conn(route)
            },
            abandon = { it.closed = true; abandoned.add(it) },
        )
        val won = race.run(listOf(Scheduled(Route.Lan(home), 0), Scheduled(Route.Relay, 0)))
        assertEquals(Route.Lan(home), won.route)
        slowMayFinish.countDown()
        // The relay finishes after the race is over, and is handed back to be
        // closed — a connection nobody will use is not left open.
        val deadline = System.nanoTime() + 5_000_000_000
        while (abandoned.isEmpty() && System.nanoTime() < deadline) Thread.sleep(5)
        assertEquals(Route.Relay, abandoned.single().route)
        assertTrue(abandoned.single().closed)
    }

    @Test
    fun `a winner cancels the paths still in flight`() {
        val relayStarted = CountDownLatch(1)
        val relayCancelled = CountDownLatch(1)
        val lanMayFinish = CountDownLatch(1)
        val race = PathRace<Conn>(
            dial = { route, cancel ->
                if (route == Route.Relay) {
                    // What a blocked socket call does: nothing, until it is
                    // closed from another thread.
                    cancel.onCancel { relayCancelled.countDown() }
                    relayStarted.countDown()
                    relayCancelled.await(5, TimeUnit.SECONDS)
                    throw IOException("socket closed")
                }
                lanMayFinish.await(5, TimeUnit.SECONDS)
                Conn(route)
            },
            abandon = {},
        )
        val thread = Thread {
            relayStarted.await(5, TimeUnit.SECONDS)
            lanMayFinish.countDown()
        }
        thread.start()
        val won = race.run(listOf(Scheduled(Route.Lan(home), 0), Scheduled(Route.Relay, 0)))
        assertEquals(Route.Lan(home), won.route)
        assertTrue(relayCancelled.await(5, TimeUnit.SECONDS), "the losing relay dial was not cancelled")
    }

    @Test
    fun `a path waiting out its head start is never dialled once the race is won`() {
        val dialled = ConcurrentLinkedQueue<Route>()
        val race = PathRace<Conn>(dial = { route, _ -> dialled.add(route); Conn(route) }, abandon = {})
        val won = race.run(
            listOf(Scheduled(Route.Lan(home), 0), Scheduled(Route.Relay, 2_000)),
        )
        assertEquals(Route.Lan(home), won.route)
        Thread.sleep(50)
        // On the machine's own network the relay learns nothing, because it
        // is never asked.
        assertEquals(listOf<Route>(Route.Lan(home)), dialled.toList())
    }

    @Test
    fun `a head start is cut short when everything before it has already failed`() {
        // No route to the home network fails at once. Waiting out the relay's
        // head start after that would be three hundred milliseconds of nothing.
        val race = PathRace<Conn>(
            dial = { route, _ ->
                if (route is Route.Lan) throw IOException("network unreachable")
                Conn(route)
            },
            abandon = {},
        )
        val started = System.nanoTime()
        val won = race.run(
            listOf(
                Scheduled(Route.Lan(home), 0),
                Scheduled(Route.Lan(dock), 0),
                Scheduled(Route.Relay, 10_000),
            ),
        )
        val ms = (System.nanoTime() - started) / 1_000_000
        assertEquals(Route.Relay, won.route)
        assertTrue(ms < 5_000, "the relay waited out a head start nothing was using: ${ms}ms")
    }

    @Test
    fun `when every path fails, a refusal from the machine is the answer and not the network`() {
        class Refused : Exception("this machine does not accept this device")
        val race = PathRace<Conn>(
            dial = { route, _ ->
                when (route) {
                    is Route.Lan -> throw Refused()
                    Route.Relay -> throw IOException("relay did not answer")
                }
            },
            abandon = {},
        )
        val lost = assertThrows<RaceLost> {
            race.run(listOf(Scheduled(Route.Lan(home), 0), Scheduled(Route.Relay, 0)))
        }
        assertEquals(2, lost.failures.size)
        val answer = lost.answer { it is Refused }
        assertTrue(answer is Refused, "a revoked phone was told the machine was unreachable: $answer")
        // And with no refusal among them there is no answer to prefer.
        assertNull(RaceLost(listOf(Route.Relay to IOException("x"))).answer { it is Refused })
    }

    @Test
    fun `a refusal on one path does not stop another from winning`() {
        // The stale-address case: a LAN address DHCP gave to the OTHER Rime
        // laptop in the house answers and refuses. The right machine, on the
        // relay, must still be reached.
        val race = PathRace<Conn>(
            dial = { route, _ ->
                if (route is Route.Lan) throw IllegalStateException("not paired")
                Thread.sleep(20)
                Conn(route)
            },
            abandon = {},
        )
        val won = race.run(listOf(Scheduled(Route.Lan(home), 0), Scheduled(Route.Relay, 0)))
        assertEquals(Route.Relay, won.route)
    }

    @Test
    fun `an empty plan is a loss with nothing in it`() {
        val lost = assertThrows<RaceLost> { PathRace<Conn>({ r, _ -> Conn(r) }, {}).run(emptyList()) }
        assertTrue(lost.failures.isEmpty())
    }

    @Test
    fun `cancelling runs a hook registered afterwards at once`() {
        val c = Cancel()
        var ran = 0
        c.onCancel { ran++ }
        c.cancel()
        c.cancel()
        assertEquals(1, ran, "a hook ran twice")
        c.onCancel { ran++ }
        assertEquals(2, ran, "a hook registered after the cancel never ran")
        assertTrue(c.cancelled)
    }

    // ---- the relay's 409 -----------------------------------------------------

    private fun conflict() = RelayException(RelayError.Upgrade("expected HTTP 101, got \"HTTP/1.1 409 Conflict\"", 409))

    @Test
    fun `a 409 is retried on exactly the contract's schedule`() {
        assertEquals(listOf(100L, 250L, 500L, 1000L, 2000L), RelayRetry.NO_DESKTOP_DELAYS_MS)
        val slept = ArrayList<Long>()
        var attempts = 0
        val e = assertThrows<RelayException> {
            RelayRetry.onNoDesktop(sleep = { slept.add(it) }) {
                attempts++
                throw conflict()
            }
        }
        assertTrue(e.noDesktopWaiting)
        assertEquals(6, attempts, "five retries, six attempts")
        assertEquals(RelayRetry.NO_DESKTOP_DELAYS_MS, slept)
    }

    @Test
    fun `a 409 that clears is a connection, not an error`() {
        var attempts = 0
        val slept = ArrayList<Long>()
        val got = RelayRetry.onNoDesktop(sleep = { slept.add(it) }) {
            attempts++
            if (attempts < 3) throw conflict()
            "joined"
        }
        assertEquals("joined", got)
        assertEquals(listOf(100L, 250L), slept)
    }

    @Test
    fun `nothing but a 409 is retried`() {
        for (e in listOf(
            RelayException(RelayError.Upgrade("expected HTTP 101, got \"HTTP/1.1 404 Not Found\"", 404)),
            RelayException(RelayError.Upgrade("its 101 did not upgrade to websocket")),
            IOException("connect timed out"),
        )) {
            var attempts = 0
            val thrown = assertThrows<Exception> {
                RelayRetry.onNoDesktop(sleep = {}) {
                    attempts++
                    throw e
                }
            }
            assertSame(e, thrown)
            assertEquals(1, attempts, "$e was retried")
        }
        assertFalse(RelayRetry.retryable(IOException("x")))
    }

    @Test
    fun `a cancelled dial stops retrying`() {
        val cancel = Cancel()
        var attempts = 0
        assertThrows<RelayException> {
            RelayRetry.onNoDesktop(cancel = cancel, sleep = { cancel.cancel() }) {
                attempts++
                throw conflict()
            }
        }
        assertEquals(1, attempts, "a race already won kept asking the relay")
    }

    @Test
    fun `the relay's own 409 line carries its status`() {
        val o = com.rimeos.remote.core.Opening.withNonce(ByteArray(16) { 1 })
        val e = assertThrows<RelayException> {
            o.check("HTTP/1.1 409 Conflict\r\nContent-Length: 0\r\n\r\n".toByteArray())
        }
        assertTrue(e.noDesktopWaiting, "${e.reason}")
        val other = assertThrows<RelayException> {
            o.check("HTTP/1.1 404 Not Found\r\n\r\n".toByteArray())
        }
        assertFalse(other.noDesktopWaiting)
    }
}
