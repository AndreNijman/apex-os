package com.apexos.remote.ui

import android.app.Application
import androidx.biometric.BiometricManager
import androidx.fragment.app.FragmentActivity
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import com.apexos.remote.core.Device
import com.apexos.remote.core.PairedMachine
import com.apexos.remote.core.StaticKey
import com.apexos.remote.core.agent.AgentSession
import com.apexos.remote.core.agent.Alert
import com.apexos.remote.core.agent.AlertWatcher
import com.apexos.remote.core.agent.AgentError
import com.apexos.remote.core.agent.Agentd
import com.apexos.remote.core.agent.GrantState
import com.apexos.remote.core.agent.Grants
import com.apexos.remote.core.agent.PrivilegeRequest
import com.apexos.remote.core.agent.Project
import com.apexos.remote.core.agent.Reply
import com.apexos.remote.core.agent.SystemGrant
import com.apexos.remote.core.agent.WorktreeStatus
import com.apexos.remote.core.agent.Hello
import com.apexos.remote.core.agent.MachineLink
import com.apexos.remote.core.Pairing
import com.apexos.remote.core.PairingException
import com.apexos.remote.core.PairingOffer
import com.apexos.remote.core.Settings
import com.apexos.remote.data.MachineRepository
import com.apexos.remote.pairing.GatedBox
import com.apexos.remote.pairing.PairingService
import com.apexos.remote.security.AppLock
import com.apexos.remote.security.AppLockRefused
import com.apexos.remote.security.KeystoreSecretBox
import com.apexos.remote.ui.term.TerminalController
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * What the screens read and what they can ask for.
 *
 * One state object and one flow rather than a graph of observable fields: every
 * screen here is a view of the same two things — the machines this phone knows
 * and what just happened — and splitting them would mean two sources that can
 * disagree about whether a pairing succeeded.
 */
data class UiState(
    val loading: Boolean = true,
    val machines: List<PairedMachine> = emptyList(),
    val settings: Settings = Settings(),
    /** `null` while the app lock has not yet been satisfied this launch. */
    val unlocked: Boolean = false,
    /** Why this device cannot gate a key, when it cannot. Shown, never hidden. */
    val gateWarning: String? = null,
    val busy: String? = null,
    val message: String? = null,
    val failure: String? = null,
    val connection: ConnectionReport? = null,
    /**
     * Whether this phone will actually show a notification.
     *
     * On screen rather than assumed, because the failure is silent: the poll
     * keeps raising alerts, `Notifier.post` keeps declining to show them, and
     * the user concludes the feature does not work.
     */
    val notificationsEnabled: Boolean = true,
    /** True while `POST_NOTIFICATIONS` has never been asked for (API 33+). */
    val notificationsUnasked: Boolean = false,
    /**
     * The last crash report, when consent allowed one to be kept (P1-060).
     *
     * Null both when there is none and when consent is off, which are the same
     * state on disk and deliberately the same state here: there is no path on
     * which a report exists and the user has not agreed to it existing.
     */
    val crash: String? = null,
    val agents: AgentUiState = AgentUiState(),
)

/**
 * What a successful connection proved.
 *
 * Kept from P1-053, and still shown when a connection is made and nothing else
 * is asked of it: the pinned key still matches, the device is still paired,
 * and the round trip came back on a frame that crossed the same path
 * everything else will.
 */
data class ConnectionReport(val machine: String, val roundTripMs: Long?)

/** The Agent Center's state for one machine. */
data class AgentUiState(
    val machine: PairedMachine? = null,
    val sessions: List<AgentSession> = emptyList(),
    val hello: Hello? = null,
    /** The session whose detail screen is open. */
    val selected: AgentSession? = null,
    val busy: String? = null,
    val failure: String? = null,
    /** Refreshed on every poll, so elapsed times move without recomputing per row. */
    val nowSeconds: Long = System.currentTimeMillis() / 1000,
    val worktrees: WorktreesUiState = WorktreesUiState(),
    val approvals: ApprovalsUiState = ApprovalsUiState(),
    /**
     * Alerts raised by the last poll and not yet shown.
     *
     * Held here rather than delivered through a callback so that the mapping
     * from a poll to a notification is a value a test can read. `:app` drains
     * it; see `Notifier`.
     */
    val alerts: List<Alert> = emptyList(),
)

/**
 * Projects, worktrees and the work in them (P1-056).
 *
 * [tooOld] is not a kind of failure: it is the answer a machine gives when its
 * APEX predates the `worktrees` verb, and it must not be drawn as an empty
 * list. It is also the ONLY answer obtainable from the image on this
 * developer's machine — 2026-09-05, against a verb that landed 2026-09-08.
 */
data class WorktreesUiState(
    val projects: List<Project> = emptyList(),
    val loading: Boolean = false,
    /** The last time this was asked, unix seconds, or 0 for never. */
    val askedSeconds: Long = 0,
    val tooOld: Boolean = false,
    val failure: String? = null,
) {
    val rows: List<WorktreeStatus> get() = projects.flatMap { it.rows }
    val everAsked: Boolean get() = askedSeconds > 0 || loading
}

/**
 * Privilege requests and grants (P1-057).
 *
 * There is no `deciding` field and no approve action, because
 * `privilege.rs:1176` refuses a decision from a paired device unconditionally.
 * See `Approvals.kt`.
 */
data class ApprovalsUiState(
    val requests: List<PrivilegeRequest> = emptyList(),
    val grants: Grants = emptyMap(),
    val systemGrants: List<Pair<SystemGrant, GrantState>> = emptyList(),
    val loading: Boolean = false,
    val busy: String? = null,
    val failure: String? = null,
) {
    val pending: List<PrivilegeRequest> get() = requests.filter { it.isPending }
}

class RemoteViewModel(application: Application) : AndroidViewModel(application) {
    private val repository = MachineRepository(application)
    private val pairing = PairingService()

    /**
     * Which sessions were in which state at the last poll (P1-058).
     *
     * ONE watcher, driven from ONE loop. It is documented as not thread-safe
     * and it genuinely is not: calling [AlertWatcher.observe] from a second
     * coroutine — a slower worktrees poll, say — would interleave its maps and
     * build exactly the race the class warns about. `startPolling` is the only
     * call site, and the worktrees it passes are the last ones fetched rather
     * than a second fetch of its own.
     */
    private val alerts = AlertWatcher()

    /**
     * Posts what the watcher raises.
     *
     * Called **from the poll loop itself**, not from a collector of
     * [AgentUiState.alerts], and that is the whole reason it is a field here
     * rather than a `LaunchedEffect` in the UI. A lifecycle-aware collector
     * stops at STOPPED — so alerts would pile up in the state while the app
     * was in the background and appear the moment the user opened it, which is
     * a notification that only arrives when you are already looking. The poll
     * runs in `viewModelScope`, which outlives the UI going away, so posting
     * from there is the only placement that notifies a phone in a pocket.
     *
     * The queue in the state is kept anyway: it is what an in-app surface
     * reads, and it is a value a test can look at, where a
     * `NotificationManager` call is not.
     */
    private val notifier = Notifier(application)

    /** Poll iterations, so a slower cadence can ride the same loop. */
    private var ticks: Long = 0

    private val _state = MutableStateFlow(UiState())
    val state: StateFlow<UiState> = _state.asStateFlow()

    /**
     * Device keys unwrapped this session, by device id.
     *
     * IN MEMORY, and that is the whole design. Unwrapping needs a `Cipher` the
     * keystore authorised, which needs an `Activity` and a biometric prompt —
     * and `PtyAttachment` calls its `connect` lambda on every backoff retry.
     * A `connect` that went through `AppLock.unlock` would produce a prompt
     * storm on a train: one dialog per reconnect, for as long as the tunnel
     * lasts, and nothing headless would ever catch it.
     *
     * So the prompt happens once, here, and the reconnect loop is handed
     * something that cannot prompt. Memory is not storage: this never reaches
     * `AppStorage`, so `InsecureStorageTest`'s walk is unaffected — and it is
     * cleared on [lock] and in [onCleared], which is what makes locking the
     * app mean something.
     */
    private val identities = HashMap<String, StaticKey>()

    /** One control connection per machine, opened on demand. */
    private val links = HashMap<String, MachineLink>()

    /** The attached terminal, when there is one. At most one at a time. */
    var terminal: TerminalController? = null
        private set

    private var poll: Job? = null

    init {
        refresh()
    }

    fun refresh() = viewModelScope.launch {
        val store = repository.load()
        val manager = BiometricManager.from(getApplication())
        val canGate = AppLock.canAuthenticate(manager)
        _state.update {
            it.copy(
                loading = false,
                machines = store.machines,
                settings = store.settings,
                gateWarning = KeystoreSecretBox.insecureFallbackReason(canGate),
                // A phone that cannot authenticate at all cannot be asked to.
                // Refusing to show the app on such a device would protect
                // nothing — the key gate is already absent, and the store holds
                // no secrets — so the lock stands down and says why.
                unlocked = it.unlocked || !canGate,
            )
        }
    }

    /**
     * The lock on the front door.
     *
     * Worth being plain about what this is and is not. The *real* protection is
     * that the device key is wrapped by a keystore key gated per use, so no
     * session can be opened without a prompt whatever any flag in this class
     * says. This is the visible lock on top of it: it stops a picked-up phone
     * showing which machines exist and what they are called. A screen is a
     * curtain and a key is a lock, and APEX Remote has both — in that order of
     * importance.
     */
    fun unlockApp(activity: FragmentActivity) = viewModelScope.launch {
        // Not before [refresh] has answered. A phone that cannot authenticate
        // at all sets `unlocked` there; prompting first would error instantly,
        // leave a failure notice on screen, and then be overruled a moment
        // later by a refresh that says no lock was needed.
        if (_state.value.loading || _state.value.unlocked) return@launch
        try {
            AppLock.confirmPresence(activity)
            _state.update { it.copy(unlocked = true, failure = null) }
        } catch (e: AppLockRefused) {
            _state.update { it.copy(failure = e.message) }
        }
    }

    /**
     * Lock the app, and mean it.
     *
     * Everything derived from an unwrapped key goes: the terminal detaches
     * (the session keeps running on the machine, which is the point), the
     * control connections close, and the keys themselves are dropped so the
     * next connection needs the prompt again. A lock that only hid the screen
     * would leave a live socket and a usable key behind it.
     */
    fun lock() {
        poll?.cancel()
        poll = null
        terminal?.close()
        terminal = null
        // Closing a `MachineLink` writes to and shuts a socket, and Android
        // kills a process that touches a socket on the main thread —
        // `StrictMode.enableDeathOnNetwork()` is on for every app targeting
        // API 11 or later and a debug build does not relax it. The keys and
        // the visible state go immediately, on this thread, because those are
        // what "locked" means; the sockets follow.
        val closing = links.values.toList()
        links.clear()
        identities.clear()
        _state.update {
            it.copy(unlocked = false, connection = null, agents = AgentUiState())
        }
        viewModelScope.launch(Dispatchers.IO) {
            closing.forEach { runCatching { it.close() } }
        }
    }

    /** Stop polling. Called when the Agent Center is no longer on screen. */
    fun leaveAgents() {
        poll?.cancel()
        poll = null
        // The watcher's history goes with it. Coming back to a machine after
        // an hour away, every session's state is news to the phone but none of
        // it is news that just happened — and `AlertWatcher` raises nothing on
        // a first poll, which is exactly the behaviour wanted here.
        _state.value.agents.machine?.let { alerts.forget(it.deviceId) }
    }

    override fun onCleared() {
        super.onCleared()
        terminal?.close()
        val closing = links.values.toList()
        links.clear()
        identities.clear()
        // `viewModelScope` is cancelled by the time this runs, so the closes
        // go to a plain thread rather than a coroutine that would never start.
        // Daemon, because a process on its way out must not be held open by a
        // socket close that is waiting on a machine that has gone.
        if (closing.isNotEmpty()) {
            Thread({ closing.forEach { runCatching { it.close() } } }, "apex-link-close").apply {
                isDaemon = true
            }.start()
        }
    }

    /**
     * Read a scanned or pasted payload, and pair with what it names.
     *
     * The offer is decoded first and on its own, because every failure here is
     * one the user can act on — the wrong code, an expired one, a code for a
     * newer APEX — and because a payload that does not decode must never reach
     * a socket.
     */
    fun pair(activity: FragmentActivity, payload: String, deviceName: String) =
        viewModelScope.launch {
            val offer = try {
                Pairing.decodeOffer(payload)
            } catch (e: PairingException) {
                _state.update { it.copy(failure = describe(e), busy = null) }
                return@launch
            }
            val now = System.currentTimeMillis()
            if (offer.expiresMs in 1..<now) {
                _state.update {
                    it.copy(
                        failure = "that pairing code has expired. Run `apex remote pair` again " +
                            "on ${offer.machine} for a fresh one.",
                        busy = null,
                    )
                }
                return@launch
            }
            _state.update { it.copy(busy = "Pairing with ${offer.machine}…", failure = null) }
            try {
                val machine = pairing.pair(
                    offer = offer,
                    deviceName = Device.checkName(deviceName),
                    // The prompt happens inside this, before a byte is sent.
                    // What comes back carries the gate the key actually has,
                    // which is what the desktop is told — a phone that promised
                    // a second factor it does not have would weaken the machine
                    // as well as itself.
                    boxFor = { deviceId -> sealingBox(activity, deviceId, offer.machine) },
                    nowMs = now,
                )
                val store = repository.remember(machine)
                _state.update {
                    it.copy(
                        machines = store.machines,
                        busy = null,
                        message = "Paired with ${machine.machine}.",
                    )
                }
            } catch (e: Exception) {
                _state.update { it.copy(busy = null, failure = describe(e)) }
            }
        }

    /**
     * Unwrap this machine's key once, open the Agent Center, and keep both.
     *
     * The prompt happens HERE and nowhere deeper, and that is not tidiness:
     * `PtyAttachment` calls its `connect` lambda on every backoff retry, so a
     * lambda that unwrapped through `AppLock` would put a biometric dialog on
     * the screen once per reconnect — a prompt storm for as long as a tunnel
     * lasts. See [identities].
     */
    fun connect(activity: FragmentActivity, machine: PairedMachine) = viewModelScope.launch {
        _state.update {
            it.copy(
                busy = "Connecting to ${machine.machine}…",
                failure = null,
                // Connecting to a DIFFERENT machine starts from an empty
                // block, not a copy of the old one. Sessions were already
                // replaced wholesale by the first poll, but worktrees, grants
                // and pending approvals are only ever fetched on request — so
                // carrying them over would leave one laptop's projects, its
                // per-project grants and its waiting root operations on the
                // screen under another laptop's name, with nothing to say they
                // were not this machine's. Undelivered alerts are the one
                // thing kept: each names the machine that raised it, so they
                // still reach the right session.
                agents = if (it.agents.machine?.deviceId == machine.deviceId) {
                    it.agents.copy(machine = machine, failure = null)
                } else {
                    AgentUiState(machine = machine, alerts = it.agents.alerts)
                },
            )
        }
        try {
            openIdentity(activity, machine)
            val link = linkFor(machine)
            val (hello, sessions) = withContext(Dispatchers.IO) {
                // `hello` first: it is the only request a mismatched client
                // can rely on, and the adapter list it carries is what the
                // start screen offers.
                val h = runCatching { link.hello() }.getOrNull()
                h to link.sessions()
            }
            _state.update {
                it.copy(
                    busy = null,
                    connection = null,
                    agents = it.agents.copy(
                        machine = machine,
                        hello = hello,
                        sessions = sessions,
                        nowSeconds = System.currentTimeMillis() / 1000,
                        failure = null,
                    ),
                )
            }
            startPolling(machine)
        } catch (e: Exception) {
            _state.update { it.copy(busy = null, failure = describeConnectFailure(machine, e)) }
        }
    }

    /**
     * Refresh the list every few seconds while it is on screen.
     *
     * Four seconds, and not faster: each poll is a control round trip over a
     * Noise session, and the thing it is watching — an agent's state — changes
     * on a human timescale. A phone that asked every second would spend
     * battery to redraw the same list.
     */
    private fun startPolling(machine: PairedMachine) {
        poll?.cancel()
        poll = viewModelScope.launch {
            while (true) {
                delay(POLL_MS)
                val link = links[machine.deviceId] ?: return@launch
                val sessions = runCatching { withContext(Dispatchers.IO) { link.sessions() } }.getOrNull()

                // Privilege requests ride the same loop at a slower cadence,
                // because `requests` is a cheap read of a small store while
                // `worktrees` runs git in every project and must never be on a
                // four-second timer. The worktrees handed to the watcher are
                // the LAST ONES FETCHED by an explicit refresh rather than a
                // fetch of this loop's own; see the note on `alerts`.
                ticks += 1
                val requests = if (ticks % REQUESTS_EVERY == 0L) {
                    runCatching { withContext(Dispatchers.IO) { link.requests() } }.getOrNull()
                } else {
                    null
                }

                val raised = if (sessions == null) {
                    // A poll that failed is not evidence that anything
                    // changed. Feeding an empty list to the watcher would
                    // "forget" every session and then re-announce all of them
                    // on the next successful poll.
                    emptyList()
                } else {
                    alerts.observe(
                        machine = machine.deviceId,
                        sessions = sessions,
                        worktrees = _state.value.agents.worktrees.rows,
                        requests = requests ?: _state.value.agents.approvals.requests,
                        nowMs = System.currentTimeMillis(),
                    ).also { raised ->
                        // Posted here and not from a collector: see the note
                        // on `notifier`. `NotificationManagerCompat.notify` is
                        // a binder call, not a socket write, so it does not
                        // belong on `Dispatchers.IO` and StrictMode has no
                        // objection to it here.
                        for (alert in raised) notifier.post(alert, machine.machine)
                    }
                }

                _state.update {
                    if (it.agents.machine?.deviceId != machine.deviceId) return@update it
                    it.copy(
                        agents = it.agents.copy(
                            sessions = sessions ?: it.agents.sessions,
                            // Advanced every poll whether or not the list
                            // changed, so elapsed times move.
                            nowSeconds = System.currentTimeMillis() / 1000,
                            selected = it.agents.selected?.let { chosen ->
                                sessions?.firstOrNull { s -> s.id == chosen.id } ?: chosen
                            },
                            approvals = if (requests == null) {
                                it.agents.approvals
                            } else {
                                it.agents.approvals.copy(requests = requests, failure = null)
                            },
                            alerts = it.agents.alerts + raised,
                        ),
                    )
                }
            }
        }
    }

    fun refreshAgents() = viewModelScope.launch {
        val machine = _state.value.agents.machine ?: return@launch
        val link = links[machine.deviceId] ?: return@launch
        try {
            val sessions = withContext(Dispatchers.IO) { link.sessions() }
            _state.update {
                it.copy(
                    agents = it.agents.copy(
                        sessions = sessions,
                        nowSeconds = System.currentTimeMillis() / 1000,
                        failure = null,
                    ),
                )
            }
        } catch (e: Exception) {
            _state.update { it.copy(agents = it.agents.copy(failure = describe(e))) }
        }
    }

    /**
     * Rewrite the Agent Center's block, optionally only while [machine] is
     * still the connected one.
     *
     * The guard is not decoration. Every loader below captures its machine,
     * hands a request to `Dispatchers.IO`, and can come back up to five
     * minutes later — `Mux.request`'s deadline. In that window the user can go
     * back, connect to a different laptop and be looking at it, and an
     * unguarded write would then put one machine's worktrees, grants and
     * pending approvals on the screen under another machine's name. The
     * four-second poll already carries this check; the slow paths need it
     * more, not less.
     *
     * Passing null means "this is not about a particular machine" — used only
     * by [drainAlerts], which empties a queue rather than describing anything.
     */
    private fun updateAgents(machine: PairedMachine? = null, block: (AgentUiState) -> AgentUiState) =
        _state.update { s ->
            if (machine != null && s.agents.machine?.deviceId != machine.deviceId) {
                s
            } else {
                s.copy(agents = block(s.agents))
            }
        }

    private fun updateWorktrees(
        machine: PairedMachine? = null,
        block: (WorktreesUiState) -> WorktreesUiState,
    ) = updateAgents(machine) { it.copy(worktrees = block(it.worktrees)) }

    private fun updateApprovals(
        machine: PairedMachine? = null,
        block: (ApprovalsUiState) -> ApprovalsUiState,
    ) = updateAgents(machine) { it.copy(approvals = block(it.approvals)) }

    // ---- worktrees (P1-056) ---------------------------------------------

    /**
     * Ask the machine about its projects and worktrees.
     *
     * Explicit, never on the four-second poll. Answering this makes the daemon
     * run git in EVERY remembered project, `merge-tree --write-tree` included,
     * and putting that on a timer would mean a phone in a pocket driving git
     * across a laptop's disk all day.
     *
     * `withContext(Dispatchers.IO)` is not optional here and not merely good
     * manners: `Mux.request` blocks for up to five minutes, so on the main
     * thread this is an ANR before `StrictMode.enableDeathOnNetwork()` gets
     * the chance to kill the process for touching a socket.
     */
    fun loadWorktrees() = viewModelScope.launch {
        val machine = _state.value.agents.machine ?: return@launch
        val link = links[machine.deviceId] ?: return@launch
        updateWorktrees(machine) { it.copy(loading = true, failure = null) }
        try {
            val projects = withContext(Dispatchers.IO) { link.projects() }
            updateWorktrees(machine) {
                it.copy(
                    projects = projects,
                    loading = false,
                    askedSeconds = System.currentTimeMillis() / 1000,
                    tooOld = false,
                    failure = null,
                )
            }
        } catch (e: AgentError) {
            // "This runtime does not have the verb" is not a failure to show as
            // one, and it is certainly not an empty list — a screen that drew
            // it as "no projects" would be reporting a version skew as a fact
            // about the machine.
            val old = Agentd.isTooOld(e)
            updateWorktrees(machine) {
                it.copy(
                    loading = false,
                    askedSeconds = System.currentTimeMillis() / 1000,
                    tooOld = old,
                    failure = if (old) null else describe(e),
                )
            }
        } catch (e: Exception) {
            updateWorktrees(machine) {
                it.copy(
                    loading = false,
                    askedSeconds = System.currentTimeMillis() / 1000,
                    failure = describe(e),
                )
            }
        }
    }

    /**
     * Open the agent working in a worktree.
     *
     * The join from a worktree to a session is the part a client running git
     * itself could not compute, so it is the daemon's `sessions` list that is
     * followed here and never a guess from a path.
     */
    fun openWorktreeSession(id: Int): AgentSession? {
        val session = _state.value.agents.sessions.firstOrNull { it.id == id } ?: return null
        selectSession(session)
        return session
    }

    // ---- approvals (P1-057) ---------------------------------------------

    /**
     * Load privilege requests and both kinds of grant.
     *
     * There is no companion that DECIDES one. `privilege.rs:1176` refuses a
     * decision from any non-local origin before it even looks at whether the
     * request is pending, so a phone can neither approve nor deny; the screen
     * says where to do it instead. See `Approvals.kt`.
     */
    fun loadApprovals() = viewModelScope.launch {
        val machine = _state.value.agents.machine ?: return@launch
        val link = links[machine.deviceId] ?: return@launch
        updateApprovals(machine) { it.copy(loading = true, failure = null) }
        try {
            val loaded = withContext(Dispatchers.IO) {
                Triple(link.requests(), link.grants(), link.systemGrants())
            }
            updateApprovals(machine) {
                it.copy(
                    requests = loaded.first,
                    grants = loaded.second,
                    systemGrants = loaded.third,
                    loading = false,
                    failure = null,
                )
            }
        } catch (e: Exception) {
            // Never an empty list: a refusal drawn as "nothing is waiting"
            // tells the user that nothing needs their attention.
            updateApprovals(machine) { it.copy(loading = false, failure = describe(e)) }
        }
    }

    /**
     * Withdraw a grant, or every grant for a project when [key] is null.
     *
     * Not retried by `MachineLink`, so a dropped connection is reported as
     * "this may not have happened" rather than silently repeated — and the
     * repeat would answer `no_such_request`, reporting failure for something
     * that succeeded.
     */
    fun revokeGrant(project: String, key: String? = null) = viewModelScope.launch {
        val machine = _state.value.agents.machine ?: return@launch
        val link = links[machine.deviceId] ?: return@launch
        updateApprovals(machine) { it.copy(busy = "Revoking…", failure = null) }
        try {
            val grants = withContext(Dispatchers.IO) { link.revoke(project, key) }
            updateApprovals(machine) { it.copy(grants = grants, busy = null) }
        } catch (e: Exception) {
            updateApprovals(machine) {
                it.copy(
                    busy = null,
                    failure = "The revoke may not have arrived: ${describe(e)}. " +
                        "Reload before trying again.",
                )
            }
        }
    }

    /** End a live system-access grant now. Same non-retry reasoning. */
    fun revokeSystemGrant(id: Int) = viewModelScope.launch {
        val machine = _state.value.agents.machine ?: return@launch
        val link = links[machine.deviceId] ?: return@launch
        updateApprovals(machine) { it.copy(busy = "Revoking…", failure = null) }
        try {
            withContext(Dispatchers.IO) { link.revokeSystemGrant(id) }
            updateApprovals(machine) { it.copy(busy = null) }
            loadApprovals()
        } catch (e: Exception) {
            updateApprovals(machine) {
                it.copy(
                    busy = null,
                    failure = "The revoke may not have arrived: ${describe(e)}. " +
                        "Reload before trying again.",
                )
            }
        }
    }

    // ---- alerts (P1-058) -------------------------------------------------

    /** Take the alerts raised since the last drain. Called by the notifier. */
    fun drainAlerts(): List<Alert> {
        val taken = _state.value.agents.alerts
        if (taken.isNotEmpty()) updateAgents { it.copy(alerts = emptyList()) }
        return taken
    }

    /**
     * Open the session a notification names, if this phone still has it.
     *
     * Returns false when it does not — a session pruned since the notification
     * was posted, or one belonging to a machine this phone is no longer
     * connected to. `SessionInfo.id` is REUSED after a prune
     * (`registry.rs:441`), so the machine is checked as well as the id and the
     * caller is expected to land on the Agent Center rather than on whatever
     * now holds that number.
     */
    fun openAlerted(machine: String, session: Int): Boolean {
        val current = _state.value.agents.machine ?: return false
        if (current.deviceId != machine) return false
        val found = _state.value.agents.sessions.firstOrNull { it.id == session } ?: return false
        selectSession(found)
        return true
    }

    fun selectSession(session: AgentSession?) =
        _state.update { it.copy(agents = it.agents.copy(selected = session)) }

    /**
     * Signal a session.
     *
     * Not retried when the connection drops — `MachineLink` refuses to, and it
     * is right: a SIGTERM delivered twice because its reply was lost is a
     * second signal into whatever the agent was doing next. The failure comes
     * back to the screen so a person can decide.
     */
    fun signal(session: AgentSession, signal: String) = viewModelScope.launch {
        val machine = _state.value.agents.machine ?: return@launch
        val link = links[machine.deviceId] ?: return@launch
        _state.update { it.copy(agents = it.agents.copy(busy = "Sending…", failure = null)) }
        try {
            withContext(Dispatchers.IO) { link.signal(session.id, signal) }
            _state.update { it.copy(agents = it.agents.copy(busy = null)) }
            refreshAgents()
        } catch (e: Exception) {
            _state.update {
                it.copy(
                    agents = it.agents.copy(
                        busy = null,
                        failure = "The signal may not have arrived: ${describe(e)}. " +
                            "Refresh before sending it again.",
                    ),
                )
            }
        }
    }

    // ---- replying to a waiting agent (P1-058) ---------------------------

    /**
     * Type a reply into a session, without attaching a terminal.
     *
     * Checked against the live list BEFORE it is sent: `SessionInfo.id` is
     * reused after a prune (`registry.rs:441`), so a reply composed from a
     * notification can arrive at a number that now belongs to a different
     * agent. `Reply.check` is what refuses that, and it refuses rather than
     * guesses — see `Reply.kt`.
     *
     * `withContext(Dispatchers.IO)` is mandatory and not stylistic:
     * `StrictMode.enableDeathOnNetwork()` is installed for every app targeting
     * API 11 or later and a debug build does not relax it, so this on the main
     * thread kills the process on the first send.
     */
    fun replyToSession(session: AgentSession, text: String) = viewModelScope.launch {
        val machine = _state.value.agents.machine ?: return@launch
        val link = links[machine.deviceId] ?: return@launch
        val target = Reply.Target(machine.deviceId, session.id, session.started)
        Reply.check(target, machine.deviceId, _state.value.agents.sessions)?.let { refusal ->
            updateAgents(machine) { it.copy(failure = refusal.message) }
            return@launch
        }
        updateAgents(machine) { it.copy(busy = "Sending…", failure = null) }
        try {
            withContext(Dispatchers.IO) { link.input(session.id, Reply.bytes(text)) }
            updateAgents(machine) { it.copy(busy = null) }
            // Asked for immediately: the agent's state changes the moment it
            // reads the line, and waiting up to four seconds to see it makes a
            // reply feel as though it went nowhere.
            refreshAgents()
        } catch (e: AgentError) {
            // The daemon ANSWERED, so nothing was typed, and saying otherwise
            // would be the worst possible error message here: told the reply
            // "may have been delivered", the user does not resend, and their
            // agent waits for ever. `protocol.rs` states this for the
            // version-skew case in its own words — "the client reports that it
            // could not deliver, and nothing has been typed" — and it holds
            // for every other kind too: `refuse_input`'s permission_denied,
            // `no_such_session`, `session_exited`, and the `Input::Failed`
            // that becomes `internal` all refuse BEFORE or instead of the
            // write.
            val message = if (Agentd.isTooOld(e)) {
                // Not a refusal. `Request::Input` landed 2026-09-08; a machine
                // older than that answers `bad_request` for the verb itself,
                // which is the same error kind a refusal uses.
                "This machine's APEX is too old to accept a typed reply. Nothing was sent. " +
                    "Run `sudo apex update` on the machine, or attach the terminal instead."
            } else {
                "Nothing was sent: ${describe(e)}"
            }
            updateAgents(machine) { it.copy(busy = null, failure = message) }
        } catch (e: Exception) {
            // No answer came back, so whether the bytes landed is genuinely
            // unknown — a socket that dropped after the write reaching the PTY
            // looks identical to one that dropped before it. Never retried: a
            // retry on the first case types the user's sentence a second time
            // into an agent that has already acted on the first.
            updateAgents(machine) {
                it.copy(
                    busy = null,
                    failure = "The connection went before the machine answered, so this reply " +
                        "may or may not have been delivered: ${describe(e)}. Check the session " +
                        "before sending it again.",
                )
            }
        }
    }

    // ---- notifications --------------------------------------------------

    /** Create the channel and record whether anything will be shown. */
    fun refreshNotificationState() {
        notifier.ensureChannel()
        _state.update {
            it.copy(
                notificationsEnabled = notifier.enabled,
                notificationsUnasked = notifier.permissionNeeded,
            )
        }
    }

    fun startAgent(cwd: String, agent: String?, worktree: String?, prompt: String?, checkpoint: Boolean = false) =
        viewModelScope.launch {
            val machine = _state.value.agents.machine ?: return@launch
            val link = links[machine.deviceId] ?: return@launch
            _state.update { it.copy(agents = it.agents.copy(busy = "Starting…", failure = null)) }
            try {
                val session = withContext(Dispatchers.IO) {
                    // 80x24 because nothing has been laid out yet; the terminal
                    // screen sends a real resize the moment it measures itself.
                    link.run(
                        cwd = cwd,
                        cols = 80,
                        rows = 24,
                        agent = agent,
                        prompt = prompt,
                        worktree = worktree,
                        checkpoint = checkpoint,
                    )
                }
                _state.update {
                    it.copy(
                        agents = it.agents.copy(
                            busy = null,
                            sessions = listOf(session) + it.agents.sessions.filterNot { s -> s.id == session.id },
                            selected = session,
                        ),
                    )
                }
            } catch (e: Exception) {
                _state.update { it.copy(agents = it.agents.copy(busy = null, failure = describe(e))) }
            }
        }

    /**
     * Attach to a session's PTY.
     *
     * A second connection, not this link's: `apex-remoted` answers a control
     * frame from a single-threaded loop that blocks while `apex-agentd`
     * thinks, and a privilege request waits as long as the person does.
     * Sharing would freeze the terminal behind a prompt somebody has not
     * answered.
     */
    fun attach(session: AgentSession): TerminalController? {
        val machine = _state.value.agents.machine ?: return null
        val identity = identities[machine.deviceId] ?: return null
        terminal?.close()
        val controller = TerminalController(
            sessionId = session.id,
            // Cannot prompt, and must not: see [identities].
            connect = { runBlocking { pairing.connect(machine, identity) } },
        )
        terminal = controller
        return controller
    }

    /**
     * Leave the terminal. The session goes on running on the machine.
     *
     * `close()` is safe to call from here: `TerminalController` puts its own
     * socket work on its io thread, for the same reason [lock] does.
     */
    fun detach() {
        terminal?.close()
        terminal = null
    }

    private suspend fun openIdentity(activity: FragmentActivity, machine: PairedMachine): StaticKey {
        identities[machine.deviceId]?.let { return it }
        val box = KeystoreSecretBox.forDevice(machine.deviceId)
        val sealed = requireNotNull(com.apexos.remote.core.Base64Url.decode(machine.sealed)) {
            "the stored key for ${machine.machine} is not base64url"
        }
        val identity = AppLock.unlock(activity, box, sealed, machine.machine)
        identities[machine.deviceId] = identity
        return identity
    }

    private fun linkFor(machine: PairedMachine): MachineLink =
        links.getOrPut(machine.deviceId) {
            MachineLink({
                val identity = identities[machine.deviceId]
                    ?: throw IllegalStateException("${machine.machine} is locked")
                runBlocking { pairing.connect(machine, identity) }
            })
        }

    /**
     * Open a session, prove it works, and hang up.
     *
     * Kept for the machines list, where "does this still work" is the question
     * being asked and nothing more is wanted.
     */
    fun ping(activity: FragmentActivity, machine: PairedMachine) = viewModelScope.launch {
        _state.update { it.copy(busy = "Connecting to ${machine.machine}…", failure = null) }
        try {
            val identity = openIdentity(activity, machine)
            val report = withContext(Dispatchers.IO) {
                pairing.connect(machine, identity).use { session ->
                    ConnectionReport(session.machine, session.measureRoundTrip())
                }
            }
            _state.update { it.copy(busy = null, connection = report, message = null) }
        } catch (e: Exception) {
            _state.update { it.copy(busy = null, failure = describeConnectFailure(machine, e)) }
        }
    }

    /**
     * A connection failure, in words that name the actual cause.
     *
     * The keystore's own exceptions do not. A key invalidated by a newly
     * enrolled fingerprint — which is deliberate, and is what
     * `setInvalidatedByBiometricEnrollment(true)` buys — surfaces as
     * `KeyPermanentlyInvalidatedException` at `Cipher.init`; a keystore that was
     * wiped surfaces as an AEAD tag mismatch, because [KeystoreSecretBox.forDevice]
     * helpfully created a fresh key that cannot open the old ciphertext. Both
     * mean the same thing to the person holding the phone, and neither says it.
     */
    private fun describeConnectFailure(machine: PairedMachine, e: Throwable): String = when {
        e is android.security.keystore.KeyPermanentlyInvalidatedException ||
            e is javax.crypto.AEADBadTagException ->
            "the key this phone used with ${machine.machine} is no longer usable — a new " +
                "fingerprint or a new screen lock invalidates it, which is the point. " +
                "Forget ${machine.machine} and pair again."
        else -> describe(e)
    }

    fun forget(machine: PairedMachine) = viewModelScope.launch {
        val store = repository.forget(machine)
        _state.update {
            it.copy(
                machines = store.machines,
                message = "${machine.machine} forgotten, and so is the key this phone used with it.",
                connection = null,
            )
        }
    }

    fun setDynamicColour(on: Boolean) = viewModelScope.launch {
        val store = repository.settings(_state.value.settings.copy(dynamicColour = on))
        _state.update { it.copy(settings = store.settings) }
    }

    // ---- crash reports (P1-060) -----------------------------------------

    /**
     * Turn keeping a crash report on or off.
     *
     * Turning it OFF also deletes whatever is already kept, and that is the
     * half worth writing down: consent withdrawn has to mean the file goes,
     * not that the next one is skipped. A report sitting on disk from before
     * somebody changed their mind is exactly the thing they changed their mind
     * about.
     */
    fun setCrashReports(on: Boolean) = viewModelScope.launch {
        val store = repository.settings(_state.value.settings.copy(crashReports = on))
        if (!on) repository.clearCrash()
        _state.update { it.copy(settings = store.settings, crash = if (on) it.crash else null) }
    }

    /** Read whatever the last crash left, for the settings dialog to offer. */
    fun refreshCrash() = viewModelScope.launch {
        _state.update { it.copy(crash = repository.crash()) }
    }

    /** Forget the kept report, once it has been read or sent. */
    fun clearCrash() = viewModelScope.launch {
        repository.clearCrash()
        _state.update { it.copy(crash = null) }
    }

    fun dismiss() = _state.update { it.copy(message = null, failure = null, connection = null) }

    private suspend fun sealingBox(
        activity: FragmentActivity,
        deviceId: String,
        machineName: String,
    ): GatedBox {
        val canGate = AppLock.canAuthenticate(BiometricManager.from(getApplication()))
        val box = KeystoreSecretBox.forDevice(deviceId, requireAuthentication = canGate)
        // `authenticationIsRequired` and not `canGate`: the first is read off
        // the key with `KeyInfo`, the second is a question asked of the system,
        // and the desktop is told the one that is true of the key protecting
        // this pairing.
        return GatedBox(
            AppLock.authoriseSeal(activity, box, machineName),
            box.authenticationIsRequired,
        )
    }

    /**
     * A sentence a person can act on.
     *
     * The protocol's own exceptions already carry one — `:core` writes its
     * messages for a reader rather than for a log — so this mostly gets out of
     * the way. What it does not do is print the exception class, which is how
     * "the machine refused this pairing code" becomes
     * "PairingException: Refused".
     */
    private fun describe(e: Throwable): String = when (e) {
        is PairingException -> e.error.message
        is AppLockRefused -> e.message ?: "the unlock was cancelled"
        else -> e.message ?: e::class.java.simpleName
    }

    private companion object {
        /**
         * Four seconds between list refreshes.
         *
         * Each one is a control round trip over a Noise session, and what it
         * watches — an agent's state — changes on a human timescale. A phone
         * polling every second would spend battery redrawing the same list.
         */
        const val POLL_MS: Long = 4_000

        /**
         * Ask for privilege requests every eighth poll — about half a minute.
         *
         * On its own loop it would be a second call site for
         * [AlertWatcher.observe], which is not thread-safe and says so. Riding
         * this one keeps that to one.
         */
        const val REQUESTS_EVERY: Long = 8
    }
}
