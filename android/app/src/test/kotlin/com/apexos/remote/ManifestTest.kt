package com.apexos.remote

import com.apexos.remote.core.agent.Handoff
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import java.io.File

/**
 * What this app is allowed to ask the phone for — asserted, not trusted.
 *
 * P1-059's fourth criterion is "no automatic access to the phone photo
 * library/filesystem beyond Android-granted selections", and a criterion about
 * something that is **absent** is only ever met by a check, because absence is
 * exactly what nobody notices changing. The moment a file-handoff transport
 * exists, the obvious way to read the chosen file is a media permission — and
 * the picker that makes it unnecessary is one line longer to write. This test
 * is what makes the cheaper right answer the one that compiles.
 *
 * The same shape covers the microphone: push-to-talk goes through the system's
 * own recogniser, so `RECORD_AUDIO` must never appear either.
 *
 * ## This test refuses to pass by failing to look
 *
 * The manifest is found by walking up from the working directory rather than
 * by a fixed relative path, and a manifest that cannot be found is a
 * **failure**, never a skip. "Permission denied is not absence" has cost this
 * project real days; a green test that silently checked nothing would be the
 * same mistake with a nicer colour.
 */
class ManifestTest {

    private val manifest: String by lazy {
        var dir: File? = File("").absoluteFile
        var found: File? = null
        while (dir != null && found == null) {
            val candidate = File(dir, "app/src/main/AndroidManifest.xml")
            val here = File(dir, "src/main/AndroidManifest.xml")
            found = when {
                candidate.isFile -> candidate
                here.isFile -> here
                else -> null
            }
            dir = dir.parentFile
        }
        requireNotNull(found) {
            "AndroidManifest.xml was not found from ${File("").absolutePath}. This test asserts " +
                "that permissions are ABSENT, so a run that could not read the manifest must " +
                "fail rather than report that it found nothing."
        }.readText()
    }

    private fun declares(permission: String): Boolean =
        manifest.contains("\"$permission\"")

    @Test
    fun `no storage or media permission is declared`() {
        // The picker route needs none of these. `ACTION_PICK_IMAGES`
        // (androidx `PickVisualMedia`) returns a URI for the one item the user
        // chose and requires no permission at all; asking for one of these
        // instead would hand the app the whole library for the life of the
        // grant, which is the "automatic access beyond Android-granted
        // selections" the criterion forbids.
        for (p in Handoff.Files.FORBIDDEN_PERMISSIONS) {
            assertFalse(
                declares(p),
                "$p is declared. The photo picker needs no permission, and this one grants " +
                    "access beyond what the user selected.",
            )
        }
    }

    @Test
    fun `RECORD_AUDIO is not declared, because this app never holds the microphone`() {
        assertFalse(
            declares("android.permission.RECORD_AUDIO"),
            "RECORD_AUDIO is declared. Push-to-talk goes through the system recogniser " +
                "(ACTION_RECOGNIZE_SPEECH), which holds the microphone itself and hands back " +
                "text — no audio enters this process and none should be able to.",
        )
        assertFalse(declares("android.permission.CAPTURE_AUDIO_OUTPUT"))
    }

    @Test
    fun `the recogniser is visible under package visibility`() {
        // Without this, `resolveActivity` answers null on API 30+ for a phone
        // that HAS a recogniser: refusal and absence confused in the direction
        // that makes a working feature look broken.
        assertTrue(
            manifest.contains("<queries>"),
            "no <queries> block, so nothing this app looks up by intent is visible to it",
        )
        assertTrue(
            manifest.contains("android.speech.action.RECOGNIZE_SPEECH"),
            "the speech recogniser is not declared in <queries>",
        )
    }

    @Test
    fun `the photo picker's fallback is visible under package visibility too`() {
        // The same defect, one feature later. `PickVisualMedia` looks for a
        // system fallback picker with `PackageManager.resolveActivity` on this
        // action — read out of activity-1.12.4's bytecode rather than assumed
        // — and package visibility filters that call on API 30 and later. A
        // null answer is not a crash; it sends the contract down
        // `ACTION_OPEN_DOCUMENT`, so the user presses "Photo" and gets a file
        // browser. Quiet, wrong, and exactly what a phone-less round cannot
        // see.
        assertTrue(
            manifest.contains("androidx.activity.result.contract.action.PICK_IMAGES"),
            "the photo picker's system fallback is not declared in <queries>, so on a phone " +
                "without the platform picker the Photo button opens a file browser",
        )
        // And the thing that must NOT have appeared alongside it. A picker is
        // the alternative to a permission, not a companion to one.
        for (p in Handoff.Files.FORBIDDEN_PERMISSIONS) {
            assertFalse(declares(p), "$p was added alongside the picker")
        }
    }

    @Test
    fun `the permissions this app does declare are the four it can justify`() {
        // A fixed set rather than a floor. A permission added without a reason
        // fails here and has to be argued for in a diff, which is the only
        // moment anybody reads the list.
        val declared = Regex("""uses-permission android:name="([^"]+)"""")
            .findAll(manifest)
            .map { it.groupValues[1] }
            .toSet()
        assertTrue(
            declared == setOf(
                // Pairing reads a QR off the desktop's screen, and works
                // without it: the payload can be pasted.
                "android.permission.CAMERA",
                // The LAN leg is a TCP connection; the relay leg a WebSocket.
                "android.permission.INTERNET",
                "android.permission.ACCESS_NETWORK_STATE",
                // An agent waiting for you is the reason to carry this app.
                "android.permission.POST_NOTIFICATIONS",
            ),
            "the declared permissions changed: $declared",
        )
    }

    @Test
    fun `the activity is not locked to one orientation and survives a rotation`() {
        // P1-060 asks for rotation and tablets/foldables. Neither is testable
        // without a device, but the two declarations that decide the outcome
        // are readable here: a locked orientation makes the criterion
        // impossible, and `configChanges` is what stops a rotation tearing down
        // the activity and taking a live terminal attachment with it.
        assertFalse(
            manifest.contains("android:screenOrientation"),
            "the activity pins an orientation, which fails P1-060's rotation and " +
                "tablet/foldable criterion by construction",
        )
        for (change in listOf("orientation", "screenSize", "screenLayout")) {
            assertTrue(
                Regex("""android:configChanges="[^"]*\b$change\b""").containsMatchIn(manifest),
                "configChanges does not handle $change, so that change recreates the activity " +
                    "and drops whatever the terminal was attached to",
            )
        }
    }

    @Test
    fun `backup stays off, so the sealed device key is never copied anywhere`() {
        assertTrue(manifest.contains("""android:allowBackup="false""""))
        assertTrue(manifest.contains("""android:fullBackupContent="false""""))
    }
}
