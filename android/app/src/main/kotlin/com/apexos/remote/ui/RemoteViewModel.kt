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
)

class RemoteViewModel(application: Application) : AndroidViewModel(application) {
    private val repository = MachineRepository(application)
    private val pairing = PairingService()

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
        links.values.forEach { runCatching { it.close() } }
        links.clear()
        identities.clear()
        _state.update {
            it.copy(unlocked = false, connection = null, agents = AgentUiState())
        }
    }

    override fun onCleared() {
        super.onCleared()
        terminal?.close()
        links.values.forEach { runCatching { it.close() } }
        identities.clear()
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
                agents = it.agents.copy(machine = machine, failure = null),
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

    fun startAgent(cwd: String, agent: String?, worktree: String?, prompt: String?) =
        viewModelScope.launch {
            val machine = _state.value.agents.machine ?: return@launch
            val link = links[machine.deviceId] ?: return@launch
            _state.update { it.copy(agents = it.agents.copy(busy = "Starting…", failure = null)) }
            try {
                val session = withContext(Dispatchers.IO) {
                    // 80x24 because nothing has been laid out yet; the terminal
                    // screen sends a real resize the moment it measures itself.
                    link.run(cwd = cwd, cols = 80, rows = 24, agent = agent, prompt = prompt, worktree = worktree)
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
    }
}
