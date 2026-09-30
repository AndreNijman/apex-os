package com.rimeos.remote.core.link

import com.rimeos.remote.core.RelayException
import java.util.concurrent.TimeUnit
import java.util.concurrent.locks.ReentrantLock
import kotlin.concurrent.withLock

/**
 * One way to reach a paired machine: an address on its network, or the relay.
 *
 * [key] is what the store remembers as the last path that worked, and it is
 * a string rather than a serialised object on purpose — `PairedMachine` is a
 * file on disk that older builds of this app also read, and a string they
 * ignore costs nothing where a new nested type would be a schema.
 */
sealed class Route {
    abstract val key: String

    /** A direct TCP connection to `host:port` on the machine's network. */
    data class Lan(val address: String) : Route() {
        override val key: String get() = "lan:$address"
    }

    /** The WebSocket relay the machine keeps a socket waiting at. */
    data object Relay : Route() {
        override val key: String get() = "relay"
    }

    /** `lan` or `relay`, for the one log line per connection. */
    val kind: String get() = if (this is Lan) "lan" else "relay"

    companion object {
        /** The route a stored [key] names, or null for one this build does not know. */
        fun parse(key: String?): Route? = when {
            key == null -> null
            key == "relay" -> Relay
            key.startsWith("lan:") && key.length > 4 -> Lan(key.substring(4))
            else -> null
        }
    }
}

/** A route and how long after the start of a connection it is tried. */
data class Scheduled(val route: Route, val delayMs: Long)

/**
 * Which paths to try, in what order, and how far apart.
 *
 * ## What this replaced, and why
 *
 * Connecting used to be strictly sequential: every stored LAN address in
 * turn, four seconds each, and only then the relay. A firewall that DROPS a
 * SYN rather than refusing it makes each of those cost the full four seconds,
 * so a phone at school with two stored home addresses waited eight seconds
 * before it even asked the relay — and then paid for the relay's TLS and
 * WebSocket on top. That was most of "it takes ages to connect".
 *
 * Now the paths RACE, and the first one to finish a Noise handshake wins:
 *
 * * **The path that worked last time goes first**, at 0 ms. Most connections
 *   are from the same place as the last one.
 * * **The other LAN addresses** start at the same moment when there is no
 *   remembered LAN path, and [LAN_STAGGER_MS] later when there is one — a
 *   head start for the address that probably works, not a queue.
 * * **The relay** starts [RELAY_HEAD_START_MS] after the LAN, so that on the
 *   machine's own network a LAN handshake — tens of milliseconds — normally
 *   finishes before the relay is dialled at all. It starts at 0 when the
 *   relay is what worked last time, or when there is no LAN address to wait
 *   for.
 *
 * A delay is a ceiling, not a promise: [PathRace] starts a waiting path
 * early the moment every path scheduled before it has already failed, so a
 * phone with no route to the home network (an immediate "network
 * unreachable") does not sit out the head start for nothing.
 *
 * ## The cost, stated
 *
 * The old code's comment said a racing client "would leak a rendezvous
 * connection every time". With the head start it mostly does not — on the
 * LAN the race is normally over before the relay is dialled — but when the
 * LAN is slow, or when the relay was the last good path, the relay does
 * learn that this phone connected at that moment even though the LAN won.
 * It learns nothing else: the rendezvous id is a hash of the machine's key,
 * and everything after the upgrade is Noise ciphertext. The trade was made
 * deliberately (docs/remote-live-contract.md §3), for connections that take
 * a third of a second instead of ten.
 */
object ConnectPlan {
    /** How long the LAN has before the relay is dialled. */
    const val RELAY_HEAD_START_MS: Long = 300

    /** How far behind a remembered LAN address the other addresses start. */
    const val LAN_STAGGER_MS: Long = 150

    fun schedule(lan: List<String>, hasRelay: Boolean, lastGood: Route?): List<Scheduled> {
        val lanRoutes = lan.distinct().map { Route.Lan(it) }
        val routes: List<Route> = lanRoutes + if (hasRelay) listOf(Route.Relay) else emptyList()
        if (routes.isEmpty()) return emptyList()
        // A remembered path that is no longer on offer — an address the
        // machine stopped listening on, a relay the pairing no longer names —
        // is not tried just because it once worked.
        val last = lastGood?.takeIf { it in routes }
        val out = ArrayList<Scheduled>(routes.size)
        if (last != null) out.add(Scheduled(last, 0))
        val lanDelay = if (last is Route.Lan) LAN_STAGGER_MS else 0L
        for (r in lanRoutes) if (r != last) out.add(Scheduled(r, lanDelay))
        if (hasRelay && last != Route.Relay) {
            out.add(Scheduled(Route.Relay, if (lanRoutes.isEmpty()) 0L else RELAY_HEAD_START_MS))
        }
        return out
    }
}

/**
 * Cancelling an attempt that is blocked in a socket call.
 *
 * A `Socket.connect` or a TLS handshake cannot be interrupted, but it can be
 * closed from another thread, which makes the blocked call throw. So an
 * attempt registers what to close as soon as it has it, and the race closes
 * the losers' sockets the moment it has a winner — which is what stops a
 * phone that already connected over Wi-Fi from also finishing a relay
 * handshake it will never use.
 */
class Cancel {
    private val lock = Any()
    private val hooks = ArrayList<() -> Unit>()

    @Volatile
    var cancelled: Boolean = false
        private set

    /** Run [hook] on cancellation, or now if that has already happened. */
    fun onCancel(hook: () -> Unit) {
        val now = synchronized(lock) {
            if (!cancelled) {
                hooks.add(hook)
                false
            } else {
                true
            }
        }
        if (now) runCatching(hook)
    }

    fun cancel() {
        val run = synchronized(lock) {
            if (cancelled) return
            cancelled = true
            hooks.toList().also { hooks.clear() }
        }
        for (h in run) runCatching(h)
    }
}

/** Every path failed. Each failure is kept, in the order the paths ended. */
class RaceLost(val failures: List<Pair<Route, Throwable>>) :
    Exception(failures.lastOrNull()?.second?.message ?: "there was no path to try") {

    /**
     * The failure to report: the first one where the machine ANSWERED, if
     * any.
     *
     * This is the old sequential loop's `reached` rule, kept. A path that
     * reached the machine and was refused — a revoked device, a protocol it
     * does not speak — is an answer, and reporting "unreachable" because the
     * relay was also down would send somebody looking for a network problem
     * on a phone that has been thrown out.
     */
    fun answer(reached: (Throwable) -> Boolean): Throwable? =
        failures.firstOrNull { reached(it.second) }?.second
}

/**
 * Try several paths at once and keep the first that works.
 *
 * Threads rather than coroutines because `:core` has none and every other
 * blocking thing here — the multiplexer's pump, the terminal's reconnect loop
 * — is a thread too. One per path, daemon, and gone when its attempt ends.
 *
 * ## A refusal does not end the race
 *
 * The old loop stopped at the first address that answered, success or not.
 * Here a refusal is recorded and the other paths keep going, and the reason
 * is a household with two Rime machines: a stored address that DHCP has since
 * handed to the OTHER laptop answers, fails the handshake (it does not hold
 * this machine's key) and would have been reported as "this device was
 * revoked" while the right machine sat on the relay. When everything fails,
 * [RaceLost.answer] still puts the refusal first.
 *
 * @param dial opens one path, blocking; registers its sockets with the
 *   [Cancel] it is handed.
 * @param abandon closes a path that finished after another had already won.
 */
class PathRace<C : Any>(
    private val dial: (Route, Cancel) -> C,
    private val abandon: (C) -> Unit,
    private val nanoTime: () -> Long = System::nanoTime,
) {
    class Won<C>(val route: Route, val result: C, val elapsedMs: Long)

    private enum class Phase { WAITING, RUNNING, FAILED, SKIPPED, WON }

    @Throws(RaceLost::class)
    fun run(plan: List<Scheduled>): Won<C> {
        if (plan.isEmpty()) throw RaceLost(emptyList())
        val lock = ReentrantLock()
        val changed = lock.newCondition()
        val phases = Array(plan.size) { Phase.WAITING }
        val cancels = List(plan.size) { Cancel() }
        val failures = ArrayList<Pair<Route, Throwable>>()
        var winner: Won<C>? = null
        var winnerAt = -1
        var ended = 0
        val start = nanoTime()

        // Whether every path due before [i] has already failed, which lets
        // [i] stop waiting for its head start to run out. Called under [lock].
        fun earlierAllFailed(i: Int): Boolean {
            val due = plan[i].delayMs
            var any = false
            for (j in plan.indices) {
                if (j == i || plan[j].delayMs >= due) continue
                any = true
                if (phases[j] != Phase.FAILED) return false
            }
            return any
        }

        for ((i, step) in plan.withIndex()) {
            val thread = Thread({
                val go = lock.withLock {
                    val deadline = start + step.delayMs * 1_000_000
                    while (winner == null && !earlierAllFailed(i)) {
                        val left = deadline - nanoTime()
                        if (left <= 0) break
                        changed.await(left, TimeUnit.NANOSECONDS)
                    }
                    if (winner != null) {
                        phases[i] = Phase.SKIPPED
                        ended++
                        changed.signalAll()
                        false
                    } else {
                        phases[i] = Phase.RUNNING
                        true
                    }
                }
                if (!go) return@Thread
                val outcome = runCatching { dial(step.route, cancels[i]) }
                val loser: C? = lock.withLock {
                    val result = outcome.getOrNull()
                    val lost = if (result != null) {
                        if (winner == null) {
                            winner = Won(step.route, result, (nanoTime() - start) / 1_000_000)
                            winnerAt = i
                            phases[i] = Phase.WON
                            null
                        } else {
                            result
                        }
                    } else {
                        phases[i] = Phase.FAILED
                        // A path that failed because it was cancelled is not
                        // evidence about the machine.
                        val e = outcome.exceptionOrNull()
                        if (e != null && !cancels[i].cancelled) failures.add(step.route to e)
                        null
                    }
                    ended++
                    changed.signalAll()
                    lost
                }
                if (loser != null) runCatching { abandon(loser) }
            }, "rime-dial-${step.route.kind}-$i")
            thread.isDaemon = true
            thread.start()
        }

        val won = lock.withLock {
            while (winner == null && ended < plan.size) changed.await()
            winner
        }
        if (won != null) {
            for ((j, c) in cancels.withIndex()) if (j != winnerAt) c.cancel()
            return won
        }
        throw RaceLost(lock.withLock { failures.toList() })
    }
}

/**
 * Retrying the relay's "no desktop is waiting" (409), and nothing else.
 *
 * ## Why it is retried at all
 *
 * The desktop keeps exactly one socket waiting at the relay. A guest that
 * joins uses it up, and for the second or so until the desktop has dialled
 * its next one, any other guest is told nobody is there — which is how a
 * phone that had just connected reported the same computer as off when it
 * opened a terminal a moment later. So a 409 is asked again, quickly, a few
 * times.
 *
 * Only a 409. A 404 is a wrong path and a 401 a deployment that wants a
 * credential, and neither gets better by asking again; neither does a relay
 * that did not answer, which already cost its own timeout.
 *
 * The schedule totals under four seconds, so a computer that really is off
 * is still reported as off promptly — and in a race, the LAN paths are being
 * tried in the meantime anyway.
 */
object RelayRetry {
    /** Milliseconds before each retry of a 409: five retries, six attempts. */
    val NO_DESKTOP_DELAYS_MS: List<Long> = listOf(100, 250, 500, 1000, 2000)

    fun retryable(e: Throwable): Boolean = e is RelayException && e.noDesktopWaiting

    /**
     * Run [attempt], retrying it after each delay in [delays] while it fails
     * with a 409 and [cancel] has not been cancelled. The last failure is the
     * one thrown.
     */
    fun <T> onNoDesktop(
        cancel: Cancel? = null,
        delays: List<Long> = NO_DESKTOP_DELAYS_MS,
        sleep: (Long) -> Unit = { Thread.sleep(it) },
        attempt: () -> T,
    ): T {
        var next = 0
        while (true) {
            try {
                return attempt()
            } catch (e: Exception) {
                if (!retryable(e) || next >= delays.size || cancel?.cancelled == true) throw e
                sleep(delays[next++])
                if (cancel?.cancelled == true) throw e
            }
        }
    }
}
