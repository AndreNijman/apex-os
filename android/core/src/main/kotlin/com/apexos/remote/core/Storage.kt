package com.apexos.remote.core

import com.apexos.remote.core.term.AccessoryKey
import com.apexos.remote.core.term.AccessoryKeys
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json

/**
 * Where a paired machine is remembered, and where its key material is not.
 *
 * P1-053's last criterion is "no secrets, private keys or session plaintext
 * are written to insecure app storage". Two things make that true here, and
 * only one of them is a promise:
 *
 * * **The shape of the record.** [PairedMachine] has no field that can hold a
 *   private key. The device's secret is present only as [PairedMachine.sealed]
 *   — whatever a [SecretBox] produced — and there is no code path from a
 *   `StaticKey` to a serialisable field. A future change that tried to store
 *   the raw key would have to add a field, which is a diff a reviewer sees.
 * * **The test.** `SecretStorageTest` seals a real keypair, writes the store,
 *   and searches the written bytes for the private key in every encoding it
 *   could plausibly take. That is a hostile check of the bytes that land on
 *   disk, not an assertion about where the code meant to put them.
 *
 * What a [SecretBox] *is* differs by platform, and that is the point of the
 * seam: on Android it is an AES-GCM key in the platform keystore created with
 * `setUserAuthenticationRequired(true)`, so opening the box needs a biometric
 * or the device credential and the key itself never enters the app's address
 * space. On a JVM — tests, and any future desktop client — it is whatever the
 * caller supplies. Neither this file nor the handshake changes between them.
 */
interface SecretBox {
    /**
     * Wrap plaintext so that only this device, with the user present, can
     * unwrap it.
     *
     * **The implementation must not retain [plaintext].** The caller wipes
     * that array the instant this returns — a private key living on in a
     * long-lived object is the thing this whole interface exists to prevent —
     * so an implementation that kept the reference would find its own copy
     * zeroed underneath it. Copy what you need before returning.
     */
    fun seal(plaintext: ByteArray): ByteArray

    /** Unwrap, or throw. A refused biometric is a throw, and that is correct. */
    fun open(ciphertext: ByteArray): ByteArray

    /** A short name for what is doing the sealing, for display and for logs. */
    val describe: String
}

/**
 * One APEX machine this device is paired with.
 *
 * P1-053 asks for "multiple APEX computers", so this is a record in a list and
 * not a singleton: everything that identifies a machine, and the device
 * identity used with *that* machine, live together. A device key per machine
 * rather than one shared key is deliberate — revoking the phone on a work
 * laptop should not hand anyone the key it uses with the home one.
 */
@Serializable
data class PairedMachine(
    /** The desktop's own id for this device: the first 16 characters of the device key. */
    @SerialName("device_id") val deviceId: String,
    /** What the machine calls itself. Display only. */
    val machine: String,
    /** The desktop's static public key, base64url. Pinned at pairing; never changes. */
    @SerialName("desktop_key") val desktopKey: String,
    /** This device's public key for this machine, base64url. */
    @SerialName("device_key") val deviceKey: String,
    /**
     * This device's *private* key for this machine, after a [SecretBox] has
     * had it. Opaque bytes, base64url. Never the key.
     */
    @SerialName("sealed_device_secret") val sealed: String,
    /** LAN hints from the pairing offer; a hint, re-learned on every connection. */
    val lan: List<String> = emptyList(),
    /** The relay to fall back to, or `null` for a LAN-only pairing. */
    val relay: String? = null,
    @SerialName("paired_ms") val pairedMs: Long,
) {
    /** The rendezvous this machine is reachable at through a relay. */
    fun rendezvousId(): String = Rendezvous.idFor(Device.checkKey(desktopKey))

    /**
     * Redacted even though the sealed blob is not a secret.
     *
     * A sealed key is still the thing an attacker needs half of, and a record
     * that printed it into logcat would be handing over that half for free.
     */
    override fun toString(): String =
        "PairedMachine(machine=$machine, deviceId=$deviceId, sealed=<${sealed.length} chars>)"
}

/**
 * What the person holding the phone has chosen about how the app looks.
 *
 * On [MachineStore] rather than in a `SharedPreferences` file, and the reason
 * is the leak test rather than tidiness. `AppStorage` is the only writer to app
 * storage precisely so that `InsecureStorageTest` can walk the whole directory
 * and account for every byte in it; a preferences file would be a second write
 * path, and one the scan never looks at. A boolean is cheap to carry here.
 */
@Serializable
data class Settings(
    /**
     * Material You instead of the APEX palette.
     *
     * Off by default. Dynamic colour is genuinely nicer on a phone whose owner
     * has chosen a wallpaper, and it is by construction *not* APEX's colours —
     * and this is the app that shows which machine is about to run something as
     * root. So it is offered, and it is not what ships.
     */
    @SerialName("dynamic_colour") val dynamicColour: Boolean = false,

    /**
     * The row of keys above the software keyboard.
     *
     * P1-055's criterion says *configurable*, and here is where the
     * configuration lives — for the same reason `dynamicColour` does, and with
     * more force: a scrollback cache, a preferences file or a `DataStore` for
     * the accessory row would each be a second write path into app storage
     * that `InsecureStorageTest`'s walk never looks at.
     *
     * Empty means "whatever this build ships", not "no keys": a stored empty
     * list and an absent key are indistinguishable after a
     * `kotlinx.serialization` default, and a row that vanished because a
     * migration dropped a field would be the worse failure. [accessoryRow]
     * resolves it.
     */
    @SerialName("accessory") val accessory: List<AccessoryKey> = emptyList(),

    /**
     * Terminal text size in scaled points.
     *
     * Not a taste setting. It is the one control that decides how many columns
     * fit, and eighty columns is what every one of these TUIs lays out for —
     * so on a phone held in portrait this is the difference between reading
     * Claude Code's output and reading a wrapped smear of it.
     */
    @SerialName("terminal_text_sp") val terminalTextSp: Float = DEFAULT_TERMINAL_TEXT_SP,

    /**
     * Keep a report on this phone when the app crashes (P1-060).
     *
     * **False by default, and the default is the criterion.** "Crash reporting
     * is consent-based" is met by a switch that starts off, not by one that
     * starts on and can be found. It gates the WRITE, not a send: with this
     * off, a crash produces nothing on disk at all, which is a stronger
     * position than a file kept back — a file that was never created cannot be
     * read by whatever reaches this phone next.
     *
     * Nothing uploads it either way. There is no reporting SDK in this app; see
     * [CrashReport] for why one would be the wrong shape here.
     *
     * On [Settings] and not in a preferences file, for the reason
     * [dynamicColour] is: `AppStorage` is the only writer to app storage so
     * that `InsecureStorageTest` can account for every byte in it.
     */
    @SerialName("crash_reports") val crashReports: Boolean = false,
) {
    /** The configured row, or the shipped one when nothing has been configured. */
    val accessoryRow: List<AccessoryKey> get() = accessory.ifEmpty { AccessoryKeys.DEFAULT }

    companion object {
        /**
         * Twelve points.
         *
         * Measured rather than chosen: at `FontFamily.Monospace` on Android the
         * advance width is 0.6 em, so eighty columns need 576 px, which is
         * inside the 1080 px short edge of every phone this app supports at
         * any reasonable density — and legible at arm's length, which a size
         * that made eighty columns fit a 720 px device would not be.
         */
        const val DEFAULT_TERMINAL_TEXT_SP: Float = 12f
    }
}

/** Every machine this device knows, and how to put one back together. */
@Serializable
data class MachineStore(
    val v: Int = VERSION,
    val machines: List<PairedMachine> = emptyList(),
    val settings: Settings = Settings(),
) {
    fun with(machine: PairedMachine): MachineStore =
        copy(machines = machines.filterNot { it.deviceId == machine.deviceId } + machine)

    fun without(deviceId: String): MachineStore =
        copy(machines = machines.filterNot { it.deviceId == deviceId })

    fun find(deviceId: String): PairedMachine? = machines.firstOrNull { it.deviceId == deviceId }

    /**
     * The device identity for a machine, unsealed.
     *
     * Returns a [StaticKey] and not bytes. The caller gets something it can
     * hand to a handshake and nothing it can write down, which is the same
     * distinction [StaticKey] itself draws and the reason this returns through
     * that interface rather than through a `ByteArray`.
     */
    fun identityFor(machine: PairedMachine, box: SecretBox): StaticKey {
        val raw = Base64Url.decode(machine.sealed)
            ?: throw DeviceException("the stored key for ${machine.machine} is not base64url")
        return InMemoryStaticKey(box.open(raw))
    }

    fun encode(): String = json.encodeToString(serializer(), this)

    companion object {
        const val VERSION = 1

        private val json = Json {
            encodeDefaults = true
            ignoreUnknownKeys = true
            prettyPrint = false
        }

        fun decode(text: String): MachineStore = json.decodeFromString(serializer(), text)

        /**
         * Build the record for a machine that has just been paired.
         *
         * The only place a device secret is ever handed to a [SecretBox], and
         * the only place `exportSecretForSealing` is called in shipped code.
         */
        fun record(
            identity: InMemoryStaticKey,
            offer: PairingOffer,
            answer: PairingAnswer,
            box: SecretBox,
            nowMs: Long,
        ): PairedMachine {
            val secret = identity.exportSecretForSealing()
            val sealed = try {
                box.seal(secret)
            } finally {
                // The one copy this code makes, wiped as soon as it is
                // sealed. It does not make the JVM's earlier copies go away —
                // `ByteArray` is not `SecretValue` and a garbage collector
                // moves things — but leaving a live reference to a private key
                // in a long-lived object would be a different and avoidable
                // problem.
                secret.fill(0)
            }
            return PairedMachine(
                deviceId = identity.deviceId(),
                machine = answer.machine ?: offer.machine,
                desktopKey = offer.key,
                deviceKey = identity.publicKeyText(),
                sealed = Base64Url.encode(sealed),
                lan = offer.lan,
                relay = offer.relay,
                pairedMs = nowMs,
            )
        }
    }
}
