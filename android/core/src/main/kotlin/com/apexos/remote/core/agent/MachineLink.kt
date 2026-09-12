package com.apexos.remote.core.agent

import com.apexos.remote.core.FrameChannel
import com.apexos.remote.core.link.Disconnected
import com.apexos.remote.core.link.Mux
import com.apexos.remote.core.link.Upload
import java.io.Closeable
import java.util.concurrent.TimeoutException

/**
 * The control plane: one connection, used for asking a machine questions.
 *
 * ## Why it is not the PTY's connection
 *
 * Because the PTY's connection is busy being a PTY. `apex-remoted` answers a
 * control frame from a single-threaded loop that **blocks while `apex-agentd`
 * thinks** — `serve.rs` — and a privilege request legitimately waits as long
 * as the person does, which is why `Mux.CONTROL_TIMEOUT_MS` is five minutes.
 * Sharing one connection would mean a list refresh could sit behind a prompt
 * somebody has gone to lunch without answering, with the terminal frozen
 * behind it.
 *
 * There is no per-device connection cap on the far side to make this
 * expensive: `apex-remoted` opens a fresh unix connection to `apex-agentd` per
 * control round trip anyway, and `net.rs` caps message size rather than
 * connections. Checked, rather than assumed.
 *
 * ## Reconnecting is a property of asking, not of a loop
 *
 * There is no reconnect *thread* here, and that is deliberate. A control
 * connection has nothing to deliver while nobody is asking — unlike a PTY,
 * which is a stream whose whole point is arriving unbidden. So a dead
 * connection is noticed on the next question and replaced then, and a phone in
 * a pocket is not reconnecting to something it is not reading.
 *
 * One retry, and only for [Disconnected]: a request that reached the daemon
 * and was refused must not be sent twice. `signal` is the case that makes this
 * more than tidiness — a `SIGTERM` delivered twice because the reply was lost
 * is a second signal into whatever the agent was doing next.
 */
class MachineLink(
    /** Opens a fresh connection. May throw; the caller sees the throw. */
    private val connect: () -> FrameChannel,
    private val onEvent: (LinkEvent) -> Unit = {},
) : Closeable {
    /** What a caller is told about the connection underneath. */
    sealed class LinkEvent {
        data class Connected(val machine: String) : LinkEvent()

        /** The connection went; the next request will make a new one. */
        data class Dropped(val cause: Throwable?) : LinkEvent()
    }

    private val lock = Any()
    private var mux: Mux? = null
    private var pump: Thread? = null

    @Volatile
    private var closed = false

    /** How many connections this link has opened. The reconnect's own evidence. */
    @Volatile
    var connections: Int = 0
        private set

    /** Whether a connection is currently up. */
    val connected: Boolean get() = synchronized(lock) { mux != null }

    // ---- the verbs ------------------------------------------------------

    fun hello(): Hello = Agentd.readHello(request(Agentd.hello()))

    /**
     * Every session the daemon knows, in the order the Agent Center draws
     * them.
     *
     * Sorted here rather than by the screen, so that a notification, a badge
     * count and a list cannot disagree about which session is first.
     */
    fun sessions(): List<AgentSession> = Order.flat(Agentd.readSessions(request(Agentd.list())))

    /** The daemon's own order, for a caller that wants to sort differently. */
    fun sessionsUnsorted(): List<AgentSession> = Agentd.readSessions(request(Agentd.list()))

    fun info(id: Int): AgentSession = Agentd.readSession(request(Agentd.info(id)))

    /**
     * Deliver a signal, by the daemon's name for it.
     *
     * Not retried on a dropped connection — see the class note. The caller is
     * told the connection went and can decide, which is the only safe place
     * for that decision: only a person knows whether the agent they meant to
     * stop is one they mind stopping twice.
     */
    fun signal(id: Int, signal: String) = Agentd.readOk(request(Agentd.signal(id, signal), retry = false))

    fun pause(id: Int) = signal(id, "stop")

    fun resume(id: Int) = signal(id, "cont")

    /** Ask politely: `SIGTERM`. */
    fun stop(id: Int) = signal(id, "term")

    fun interrupt(id: Int) = signal(id, "int")

    /**
     * Start a session, and get it back.
     *
     * Never retried, for the obvious reason: a `run` whose reply was lost may
     * have started an agent, and a second attempt would start a second one in
     * the same directory. `cwd` must be absolute — `RunRequest` says so — and
     * that is checked here rather than discovered as a daemon error.
     */
    fun run(
        cwd: String,
        cols: Int,
        rows: Int,
        agent: String? = null,
        prompt: String? = null,
        worktree: String? = null,
        checkpoint: Boolean = false,
    ): AgentSession {
        require(cwd.startsWith("/")) { "a working directory must be absolute, and `$cwd` is not" }
        return Agentd.readSession(
            request(Agentd.run(cwd, cols, rows, agent, prompt, worktree, checkpoint), retry = false),
        )
    }

    /**
     * Type into a live session, without attaching.
     *
     * **Never retried**, and this is the strongest case for that on the whole
     * link: `input` is the only verb here that is not idempotent in the
     * ordinary sense. A `run` whose reply was lost may have started an agent;
     * an `input` whose reply was lost HAS put the bytes on the terminal, and a
     * retry types the user's sentence a second time — into an agent that has
     * by then acted on the first copy. Better to tell the caller the
     * connection went and let them look.
     *
     * Bytes go through [Reply.bytes], never straight from a text field: the
     * daemon appends no terminator, so a reply sent as typed is a reply the
     * agent never receives.
     */
    fun input(id: Int, data: String) =
        Agentd.readOk(request(Agentd.input(id, data), retry = false))

    /**
     * Per-worktree status for every remembered project, or for one slug.
     *
     * Retried on a dropped connection like every other question here, and this
     * one deserves the note: answering it makes the daemon run git in every
     * remembered project, `merge-tree --write-tree` included. That writes
     * objects, which sounds like an action — but it writes only unreferenced
     * ones into the object database to answer "would this merge", and the
     * answer to asking twice is the same answer. It is a question.
     *
     * It is also slow, which is why it is not folded into the Agent Center's
     * four-second poll. `Mux.CONTROL_TIMEOUT_MS` is five minutes, matching
     * `apex-remoted`'s own, so a large repository has room.
     */
    /**
     * Hand a file to a session whose bytes are on this phone (P1-059).
     *
     * **On a connection of its own**, and this is the one verb here that does
     * not travel on the connection this class holds. Two reasons, both about
     * the connection rather than about the file: `receive` TAKES A CHANNEL
     * OVER, so it cannot go through [request] at all — `apex-remoted` refuses
     * a takeover verb on channel zero by name — and a multi-megabyte upload
     * ahead of everything else in [Mux]'s strict FIFO is a session list that
     * arrives when the photo finishes. [com.apexos.remote.core.link.Upload]
     * has the whole account.
     *
     * Blocking, like everything else here, and for longer than anything else
     * here: callers run it inside `withContext(Dispatchers.IO)`.
     */
    fun upload(
        id: Int,
        name: String,
        len: Long,
        source: () -> java.io.InputStream,
        onProgress: (Long) -> Unit = {},
    ): Upload.Landed = Upload.send(connect, id, name, len, source, onProgress)

    fun worktrees(project: String? = null): List<WorktreeStatus> =
        Agentd.readWorktrees(request(Agentd.worktrees(project)))

    /** The same, grouped into the projects the daemon walked. */
    fun projects(): List<Project> = Project.group(worktrees())

    // ---- approvals (P1-057) ---------------------------------------------
    //
    // There is no `decide` here and there must not be one. See `Approvals.kt`.

    fun requests(): List<PrivilegeRequest> = Agentd.readRequests(request(Agentd.requests()))

    /** Only the ones still waiting on a human — at the machine, not here. */
    fun pendingRequests(): List<PrivilegeRequest> = requests().filter { it.isPending }

    fun grants(): Grants = Agentd.readGrants(request(Agentd.grants()))

    fun systemGrants(): List<Pair<SystemGrant, GrantState>> =
        Agentd.readSystemGrants(request(Agentd.systemGrants()))

    /**
     * Withdraw a per-project grant, or all of them for the project.
     *
     * **Not retried**, and that is not tidiness. `revoke` without a key
     * removes every grant for a project, and a reply lost after the daemon
     * acted would have the retry answer `no_such_request` — reporting a
     * failure for something that succeeded, which on a screen about authority
     * is the wrong way round. The caller is told the connection went.
     */
    fun revoke(project: String, key: String? = null): Grants =
        Agentd.readGrants(request(Agentd.revoke(project, key), retry = false))

    /** End a live system-access grant now. Not retried, for the reason above. */
    fun revokeSystemGrant(id: Int): List<Pair<SystemGrant, GrantState>> =
        Agentd.readSystemGrants(request(Agentd.revokeSystemGrant(id), retry = false))

    // ---- the connection -------------------------------------------------

    /**
     * Send one request, opening or replacing the connection as needed.
     *
     * [retry] is what separates a question from an instruction. A question
     * that was lost costs nothing to ask again; an instruction that was lost
     * may or may not have been carried out, and only the caller knows whether
     * repeating it is safe.
     */
    @Throws(Disconnected::class, AgentError::class, TimeoutException::class)
    fun request(line: String, retry: Boolean = true): String {
        check(!closed) { "this link is closed" }
        return try {
            live().request(line)
        } catch (e: Disconnected) {
            drop(e)
            if (!retry) throw e
            // One more, on a connection made for this attempt. If that fails
            // the throw reaches the caller: two dead connections in a row is
            // a machine that is not there, not a link that needs another go.
            live().request(line)
        }
    }

    private fun live(): Mux {
        synchronized(lock) {
            mux?.let { return it }
        }
        // Outside the lock: opening a connection is a handshake over a socket
        // and holding a monitor across it would block every other caller,
        // including the one trying to close this link.
        val channel = connect()
        val m = Mux(channel, Listener())
        val thread = Thread({ m.pump() }, "apex-link-${channel.machine}")
        thread.isDaemon = true
        val install = synchronized(lock) {
            if (closed || mux != null) {
                null
            } else {
                mux = m
                pump = thread
                connections++
                m
            }
        }
        if (install == null) {
            // Two callers raced, or the link closed while this one was
            // connecting. The loser's connection is closed rather than leaked.
            runCatching { m.close() }
            runCatching { channel.close() }
            synchronized(lock) { mux }?.let { return it }
            throw Disconnected("this link to ${channel.machine} closed while connecting")
        }
        thread.start()
        onEvent(LinkEvent.Connected(channel.machine))
        return install
    }

    private fun drop(cause: Throwable?) {
        val going = synchronized(lock) {
            val m = mux ?: return
            mux = null
            pump = null
            m
        }
        runCatching { going.close() }
        onEvent(LinkEvent.Dropped(cause))
    }

    private inner class Listener : Mux.Listener {
        // A control connection carries no channels: nothing is ever opened on
        // it, so data and close cannot arrive. They are not errors, and they
        // are not silently swallowed either — they simply cannot happen, and
        // a build where they do has a multiplexer bug rather than a link one.
        override fun onData(channel: UInt, bytes: ByteArray) = Unit

        override fun onClose(channel: UInt, reason: String) = Unit

        override fun onDisconnect(cause: Throwable?) {
            // The connection ended on its own — the machine went away, or slept.
            // Cleared here so the next request opens a new one rather than
            // failing against a corpse.
            val ended = synchronized(lock) {
                val m = mux ?: return
                mux = null
                pump = null
                m
            }
            runCatching { ended.close() }
            onEvent(LinkEvent.Dropped(cause))
        }
    }

    override fun close() {
        closed = true
        drop(null)
    }
}
