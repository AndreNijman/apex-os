package com.apexos.remote.update

import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInstaller
import android.net.Uri
import android.os.Build
import android.provider.Settings
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.apexos.remote.BuildConfig
import com.apexos.remote.core.update.UpdateOffer
import com.apexos.remote.core.update.UpdateRejected
import com.apexos.remote.core.update.Updates
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.InputStream
import java.net.HttpURLConnection
import java.net.URL

/**
 * Keeping this app from drifting away from the machines it talks to.
 *
 * ## Why this app self-updates when the desktop apps must not
 *
 * On 2026-09-11 Andre ruled that the ChatGPT and Claude **desktop** apps must
 * not self-update: they are baked into the APEX image, and `sudo apex update`
 * already moves them, so a second updater would compete with a working one.
 *
 * None of that reasoning reaches a phone. The phone is not running APEX-OS,
 * the APK is in no image, and `apex update` has no way to touch it. There is
 * no other path — so the choice here is not "one updater or two", it is "an
 * updater or an app that only moves when its owner remembers to go and look".
 * A client that silently falls behind the OS is the failure this whole unit
 * exists to prevent. `docs/android-app.md` records the distinction so that the
 * next reader does not think the desktop rule was broken by accident.
 *
 * ## Nothing happens silently
 *
 * Android will not let an app replace itself without the user seeing it, and
 * that is the right behaviour rather than an obstacle. `REQUEST_INSTALL_PACKAGES`
 * only earns the right to ASK; the system still draws the install prompt, and
 * on GrapheneOS it draws it too. So the flow is: notice, offer, download,
 * verify, and then hand the system a session it shows the user.
 *
 * ## Failing safe
 *
 * A phone with no signal, a captive portal, a rate limit, a GitHub outage: all
 * of these leave [state] at [UpdateUi.Idle] and say nothing at all. The user
 * gets an app that works with what it has. The only thing that speaks up
 * without being asked is a download whose bytes did not match the published
 * checksum, because that one must never be quiet.
 *
 * ## What is NOT proved here
 *
 * Every line below the download is Android API glue — `PackageInstaller`, the
 * permission, the prompt — and **no test in this repository executes it**,
 * because there is no device and no emulator. The parts that can be wrong
 * without anybody noticing — which release to look at, whether a build is
 * newer, whether the bytes are the published ones — are in `:core` and are
 * tested there against the real shape of this repository's Releases page. This
 * file is deliberately thin so that the untested half stays small.
 */
class AppUpdater(private val context: Context, private val repo: String = APEX_OS_REPO) {

    var state by mutableStateOf<UpdateUi>(UpdateUi.Idle)
        private set

    /** Checked at most once per process. Never persisted — see below. */
    private var checked = false

    /**
     * Look for a newer build. Returns without a word if there is not one, or
     * if anything at all went wrong.
     *
     * **No "last checked" timestamp is stored anywhere.** A timestamp would be
     * a second write path into app storage, which `InsecureStorageTest` walks
     * and accounts for byte by byte and `no-second-write-path.sh` forbids. The
     * cost of not having one is one HTTP request per app launch, against an
     * endpoint that is cached and a few kilobytes; the cost of having one is a
     * file that the storage rules would have to be relaxed for.
     */
    suspend fun check() {
        if (checked) return
        checked = true
        val found = withContext(Dispatchers.IO) {
            try {
                val listing = fetchText("https://api.github.com/repos/$repo/releases?per_page=30")
                    ?: return@withContext null
                val release = Updates.newestAndroidRelease(listing) ?: return@withContext null
                val metaUrl = Updates.metadataUrl(release) ?: return@withContext null
                val meta = Updates.parseMetadata(fetchText(metaUrl) ?: return@withContext null)
                    ?: return@withContext null
                Updates.offer(BuildConfig.VERSION_CODE, release, meta)
            } catch (e: Throwable) {
                // Everything. A phone that cannot reach GitHub is an ordinary
                // phone, and an app that reported that as a problem would be
                // crying wolf at every airport and every train tunnel.
                null
            }
        }
        if (found != null) state = UpdateUi.Available(found)
    }

    /** True when Android would let this app ask to install one. */
    fun mayInstall(): Boolean =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            context.packageManager.canRequestPackageInstalls()
        } else {
            true
        }

    /**
     * The Settings page that grants it — the app cannot grant it itself, and
     * there is no dialog for it. Sending the user straight to the right page
     * is the difference between "turn this on in Settings somewhere" and one
     * tap.
     */
    fun sourcesSettings(): Intent =
        Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:${context.packageName}"))

    /**
     * Download [offer], prove it is the published file, and hand it to the
     * system installer.
     *
     * The order is the whole point. The bytes are streamed **into the
     * installer session** while being hashed, and `commit` — the call that
     * shows the user a prompt and can replace this app — happens only after
     * the hash agrees. A mismatch abandons the session, so the rejected bytes
     * are discarded by the system and never become a file this app owns.
     *
     * That is also why there is no download directory, no `FileProvider` and
     * no `content://` URI anywhere in this app: an APK that never lands in app
     * storage cannot be a second write path, cannot be left behind, and cannot
     * be swapped between the check and the install.
     */
    suspend fun install(offer: UpdateOffer) {
        state = UpdateUi.Working("Downloading ${offer.versionName}…")
        val outcome = withContext(Dispatchers.IO) {
            val installer = context.packageManager.packageInstaller
            val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL)
            params.setAppPackageName(context.packageName)
            var sessionId = -1
            try {
                sessionId = installer.createSession(params)
                installer.openSession(sessionId).use { session ->
                    openStream(offer.apkUrl).use { input ->
                        session.openWrite(WRITE_NAME, 0, offer.sizeBytes).use { out ->
                            Updates.copyVerified(input, out, offer)
                            // Flushed through the session's own API: a plain
                            // close is not documented to have reached the
                            // system's copy, and committing a partially
                            // written session fails in a way that reads as a
                            // corrupt download.
                            session.fsync(out)
                        }
                    }
                    session.commit(statusSender(sessionId))
                }
                null
            } catch (e: UpdateRejected) {
                if (sessionId >= 0) runCatching { installer.abandonSession(sessionId) }
                // The one loud failure. See the class comment.
                e.message
            } catch (e: Throwable) {
                if (sessionId >= 0) runCatching { installer.abandonSession(sessionId) }
                "The update could not be downloaded. Nothing was installed and the app is " +
                    "unchanged; it will try again next time you open it."
            }
        }
        state = if (outcome == null) {
            // Not "installed". `commit` hands the system a prompt the user has
            // not answered yet, and claiming success before they have is how a
            // UI comes to disagree with the phone.
            UpdateUi.Working("Waiting for Android's install prompt…")
        } else {
            UpdateUi.Problem(outcome)
        }
    }

    /** Put the banner away without updating. It returns on the next launch. */
    fun dismiss() {
        state = UpdateUi.Idle
    }

    // ── Plumbing ─────────────────────────────────────────────────────────────

    private fun statusSender(sessionId: Int): android.content.IntentSender {
        val intent = Intent(UpdateInstallReceiver.ACTION).setPackage(context.packageName)
        // MUTABLE, and it has to be: `PackageInstaller` fills this intent in
        // with the session's status and, when the user has to be asked, with
        // the confirmation intent itself. An immutable one arrives empty and
        // the install simply never happens.
        val flags = PendingIntent.FLAG_UPDATE_CURRENT or
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) PendingIntent.FLAG_MUTABLE else 0
        return PendingIntent.getBroadcast(context, sessionId, intent, flags).intentSender
    }

    private fun connect(url: String): HttpURLConnection {
        val c = URL(url).openConnection() as HttpURLConnection
        c.connectTimeout = 15_000
        c.readTimeout = 30_000
        c.instanceFollowRedirects = true
        // GitHub asks for this header and answers differently without it.
        c.setRequestProperty("Accept", "application/vnd.github+json")
        c.setRequestProperty("User-Agent", "apex-remote-android")
        return c
    }

    private fun fetchText(url: String): String? {
        val c = connect(url)
        return try {
            if (c.responseCode != 200) return null
            // Bounded. A listing is a few kilobytes; anything that keeps coming
            // is not a listing, and reading it all into a string on a phone to
            // find that out would be the last thing this process did.
            c.inputStream.readBounded(MAX_TEXT_BYTES)
        } catch (e: Throwable) {
            null
        } finally {
            c.disconnect()
        }
    }

    private fun openStream(url: String): InputStream {
        val c = connect(url)
        // `Accept` is overridden for the asset itself: with the JSON media type
        // GitHub answers the API's description of the asset rather than its
        // bytes, which would download a few hundred bytes of JSON and fail the
        // length check with a message about a short download.
        c.setRequestProperty("Accept", "application/octet-stream")
        if (c.responseCode != 200) {
            c.disconnect()
            throw java.io.IOException("the download answered HTTP ${c.responseCode}")
        }
        return c.inputStream
    }

    private fun InputStream.readBounded(limit: Int): String? {
        val buffer = ByteArray(8192)
        val out = StringBuilder()
        var total = 0
        while (true) {
            val n = read(buffer)
            if (n < 0) break
            total += n
            if (total > limit) return null
            out.append(String(buffer, 0, n, Charsets.UTF_8))
        }
        return out.toString()
    }

    companion object {
        const val APEX_OS_REPO = "AndreNijman/apex-os"
        private const val WRITE_NAME = "apex-remote.apk"
        private const val MAX_TEXT_BYTES = 2 * 1024 * 1024
    }
}

/** What the one update row in the UI is showing. */
sealed interface UpdateUi {
    /** Nothing to say. This is also what every network failure looks like. */
    data object Idle : UpdateUi

    /** A newer build exists and the user has not decided yet. */
    data class Available(val offer: UpdateOffer) : UpdateUi

    /** Something is happening and the user should not press it twice. */
    data class Working(val text: String) : UpdateUi

    /**
     * Said out loud, because it is not a network problem.
     *
     * A checksum that did not match is the only thing this app volunteers a
     * complaint about. Everything quieter than that stays [Idle].
     */
    data class Problem(val text: String) : UpdateUi
}
