package com.apexos.remote.ui

import android.app.Application
import androidx.biometric.BiometricManager
import androidx.fragment.app.FragmentActivity
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import com.apexos.remote.core.Device
import com.apexos.remote.core.PairedMachine
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
import kotlinx.coroutines.Dispatchers
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
)

/**
 * What a successful connection proved.
 *
 * P1-054 and P1-055 turn this into an agent list and a terminal. Until they
 * do, saying which machine answered and how long it took is the honest end of
 * the story — and it is not nothing: it means the pinned key still matches, the
 * device is still paired, and the round trip came back on a frame that crossed
 * the same path everything else will.
 */
data class ConnectionReport(val machine: String, val roundTripMs: Long?)

class RemoteViewModel(application: Application) : AndroidViewModel(application) {
    private val repository = MachineRepository(application)
    private val pairing = PairingService()

    private val _state = MutableStateFlow(UiState())
    val state: StateFlow<UiState> = _state.asStateFlow()

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

    fun lock() = _state.update { it.copy(unlocked = false, connection = null) }

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
     * Open a session, prove it works, and hang up.
     *
     * The prompt happens here and not inside the socket code: unwrapping the
     * device key needs an authorised `Cipher`, and getting one needs an
     * `Activity`. Everything after it is off the main thread.
     */
    fun connect(activity: FragmentActivity, machine: PairedMachine) = viewModelScope.launch {
        _state.update { it.copy(busy = "Connecting to ${machine.machine}…", failure = null) }
        try {
            val box = KeystoreSecretBox.forDevice(machine.deviceId)
            val sealed = requireNotNull(com.apexos.remote.core.Base64Url.decode(machine.sealed)) {
                "the stored key for ${machine.machine} is not base64url"
            }
            val identity = AppLock.unlock(activity, box, sealed, machine.machine)
            val report = withContext(Dispatchers.IO) {
                pairing.connect(machine, identity).use { session ->
                    // A completed handshake and "frames cross this path" are
                    // not the same claim, and only the second one is worth
                    // showing somebody. So: ping, and wait for the answer.
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
}
