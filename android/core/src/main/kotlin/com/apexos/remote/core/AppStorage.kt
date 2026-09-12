package com.apexos.remote.core

import java.io.File

/**
 * The **only** thing that writes to the app's private storage.
 *
 * ## Why this is in `:core` and not in the Android module
 *
 * P1-053's last criterion is that no secret, private key or session plaintext
 * reaches insecure app storage. A claim like that is worth exactly as much as
 * the test behind it, and a test is only possible if there is a single, named,
 * platform-free thing that does the writing. So the persistence lives here,
 * over a plain [File] directory, and the Android side is a two-line adapter
 * that hands it `context.filesDir`.
 *
 * That gives `InsecureStorageTest` something it could not otherwise have: a
 * real directory it can fill by driving a real pairing and a real session, and
 * then walk byte by byte looking for material that should not be there. A test
 * that could only inspect a string returned by [MachineStore.encode] would miss
 * a second file, a temporary file left behind by a crash, and a debug log —
 * which are three of the likelier ways a key actually escapes.
 *
 * ## The rule that makes the test cover the whole surface
 *
 * **Nothing in `:app` opens a file of its own.** Every byte that lands in
 * `filesDir` goes through this class. That is why the display preferences are a
 * field on [MachineStore] rather than a `SharedPreferences` file: a second
 * write path would be a second place for something to leak, and it would be one
 * the scan never looks at. `tools/no-second-write-path.sh` is the enforcement,
 * because a rule with no check is a comment.
 *
 * ## What is deliberately absent
 *
 * No cache of session output, no transcript, no "last command" history. A
 * terminal's scrollback lives in memory and dies with the process. It would be
 * genuinely convenient to persist it, and it is exactly the "session plaintext"
 * the criterion names, so the convenience is refused rather than encrypted:
 * material that is never written cannot be written wrongly.
 */
class AppStorage(private val directory: File) {

    /** The one file. Public so a test can name it; there is nothing secret in the path. */
    val storeFile: File = File(directory, STORE_NAME)

    /**
     * Read the store, or an empty one.
     *
     * A file that will not parse produces an empty store rather than an
     * exception. A corrupt store is not a reason to lose the app, and it *is* a
     * reason to make the user pair again — which takes ten seconds and produces
     * a fresh key. Silently starting empty is the honest outcome, because
     * nothing in a damaged file can be trusted to name a machine correctly.
     */
    fun load(): MachineStore {
        if (!storeFile.exists()) return MachineStore()
        return try {
            MachineStore.decode(storeFile.readText())
        } catch (_: Exception) {
            MachineStore()
        }
    }

    /**
     * Replace the store.
     *
     * Written beside and renamed, so a kill mid-write leaves the previous list
     * rather than half of the new one — a half-written store is a phone that
     * can no longer reach anything. The scratch file is deleted on a failure
     * rather than left: a `.new` holding a sealed key is still a file the scan
     * would have to account for, and there is no reason for one to survive.
     */
    fun save(store: MachineStore) {
        directory.mkdirs()
        val scratch = File(directory, "$STORE_NAME.new")
        try {
            scratch.writeText(store.encode())
            if (!scratch.renameTo(storeFile)) {
                // `renameTo` is allowed to fail without saying why. Falling
                // back to a copy keeps the data; what it loses is atomicity,
                // and that is a smaller loss than losing the pairing.
                storeFile.writeText(store.encode())
            }
        } finally {
            scratch.delete()
        }
    }

    /** Read, change, write. The only shape in which `:app` is expected to use this. */
    fun update(block: (MachineStore) -> MachineStore): MachineStore =
        block(load()).also { save(it) }

    companion object {
        /** One file, named for what it holds rather than for the app. */
        const val STORE_NAME = "machines.json"
    }
}
