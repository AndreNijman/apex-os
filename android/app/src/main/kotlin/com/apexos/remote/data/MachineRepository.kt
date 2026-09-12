package com.apexos.remote.data

import android.content.Context
import com.apexos.remote.core.AppStorage
import com.apexos.remote.core.MachineStore
import com.apexos.remote.core.PairedMachine
import com.apexos.remote.core.Settings
import com.apexos.remote.security.KeystoreSecretBox
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * The adapter, and deliberately nothing more.
 *
 * The persistence itself is [AppStorage] in `:core`, over a plain directory.
 * All this class adds is the one Android fact — *which* directory — and a move
 * off the main thread. That split is what lets `InsecureStorageTest` drive a
 * real pairing and a real session against a real folder and then walk every
 * byte in it; a repository that did its own file handling here would be
 * untestable on a machine with no device, which is every machine this project
 * builds on.
 *
 * `filesDir` and not `getExternalFilesDir`: external storage is world-readable
 * by design. Not `SharedPreferences` either, for the display settings or for
 * anything else — [Settings] rides on [MachineStore] precisely so there is one
 * file and one writer. `tools/no-second-write-path.sh` fails the build if a
 * second one appears, and this file is its single exemption.
 */
class MachineRepository(context: Context) {
    private val storage = AppStorage(context.filesDir)

    // ── crash reports (P1-060) ──────────────────────────────────────────────
    //
    // Through the same adapter as everything else, because `AppStorage` is the
    // only writer to app storage and `no-second-write-path.sh` is what keeps
    // that true. A handler that opened a file of its own would be a second
    // write path AND one created at the worst possible moment: while the
    // process is being torn down, in the least exercised code in any app.
    //
    // `saveCrash` is NOT a suspend function and does not hop to
    // `Dispatchers.IO`, which is the one place this file departs from its own
    // rule. An uncaught-exception handler runs on the dying thread and the
    // process will not outlive it; launching the write on another dispatcher
    // would hand it to a coroutine that never gets scheduled, and the report
    // would be lost exactly when it was wanted. It writes a few kilobytes to
    // the app's own private directory, which is not a socket — StrictMode's
    // death penalty here is for network, and disk on a dying thread costs a
    // process that is already gone nothing.

    /**
     * Keep a redacted report, if the user has said so.
     *
     * @return whether anything was written. Consent and well-formedness are
     *   both checked inside [AppStorage.saveCrash], not here.
     */
    fun saveCrashBlocking(report: String, consented: Boolean): Boolean =
        storage.saveCrash(report, consented)

    /**
     * Whether the user has agreed to a report being kept, read blocking.
     *
     * Here and not at the handler for the reason `no-second-write-path.sh`
     * exists, and the script caught the first attempt: naming `filesDir`
     * anywhere but this file is a second place storage is reached from, and a
     * read today is a write tomorrow.
     *
     * `AppStorage.load` answers with an empty store — and so with
     * `crashReports = false` — when the file is missing or will not parse, so
     * the failure mode of this read is "write nothing".
     */
    fun crashConsentBlocking(): Boolean = storage.load().settings.crashReports

    /** The last report, for the screen that offers to share it. */
    suspend fun crash(): String? = withContext(Dispatchers.IO) { storage.loadCrash() }

    /** Forget it, once it has been read or consent has been withdrawn. */
    suspend fun clearCrash() = withContext(Dispatchers.IO) { storage.clearCrash() }

    suspend fun load(): MachineStore = withContext(Dispatchers.IO) { storage.load() }

    suspend fun save(store: MachineStore) = withContext(Dispatchers.IO) { storage.save(store) }

    suspend fun remember(machine: PairedMachine): MachineStore =
        withContext(Dispatchers.IO) { storage.update { it.with(machine) } }

    suspend fun settings(settings: Settings): MachineStore =
        withContext(Dispatchers.IO) { storage.update { it.copy(settings = settings) } }

    /**
     * Forget a machine, and the keystore key that would have unsealed it.
     *
     * Both halves, in that order, because a record with no key is a row the
     * user can delete again while a key with no record is invisible. The
     * sealed bytes become noise the moment the alias is gone, which is the
     * property that makes "forget this computer" mean something stronger than
     * removing a line from a file.
     */
    suspend fun forget(machine: PairedMachine): MachineStore = withContext(Dispatchers.IO) {
        val after = storage.update { it.without(machine.deviceId) }
        KeystoreSecretBox.forget(machine.deviceId)
        after
    }
}
