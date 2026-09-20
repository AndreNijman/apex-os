package com.apexos.remote.core.update

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import org.bouncycastle.crypto.digests.SHA256Digest
import java.io.InputStream
import java.io.OutputStream

/**
 * Finding out whether a newer build of this app exists, and proving that the
 * file downloaded is that build.
 *
 * ## Why there is an updater at all
 *
 * The phone is not running APEX-OS. The APK is not in the image, `apex update`
 * cannot reach a phone, and there is no app store in this picture. So unlike
 * the desktop AI apps — which must NOT self-update, because `sudo apex update`
 * is a real update path that a second one would compete with — this app has no
 * other path. Without this file the only way a phone ever moves forward is its
 * owner remembering to go and look, and a client that drifts from the OS is
 * exactly the failure this whole unit exists to prevent.
 *
 * ## Everything here is pure JVM, on purpose
 *
 * Deciding *whether* to update, and proving a downloaded file is the right
 * one, are the two parts that can be wrong in ways nobody notices. They are
 * therefore in `:core`, where they are tested on a JVM with no device. The
 * Android half — `PackageInstaller`, the permission, the prompt the user sees
 * — is glue in `:app` and holds no decisions.
 *
 * ## The fail-safe rule
 *
 * A phone that cannot reach GitHub is a normal phone, not a broken one. Every
 * function here reports "nothing to offer" by returning `null` rather than by
 * throwing, for every condition that is merely an absence: no Android release
 * yet, a release with no metadata, a build that is not newer. The one thing
 * that throws is [UpdateRejected], and it is never about reachability — it is
 * the downloaded bytes failing to be what the release said they would be.
 */
object Updates {

    /**
     * `ignoreUnknownKeys`, deliberately, and in both directions.
     *
     * The GitHub API response carries around forty fields this app has no
     * interest in, and a future release metadata file may carry one this build
     * has never heard of. A strict parser would turn "the release flow added a
     * field" into "every installed phone stopped seeing updates", which is the
     * same drift failure one level up.
     */
    private val json = Json {
        ignoreUnknownKeys = true
        isLenient = false
    }

    /** Tags this project's Android releases wear: `android-v<versionCode>`. */
    private val ANDROID_TAG = Regex("""^android-v(\d{1,9})$""")

    /**
     * The newest Android release in a GitHub releases listing, or `null`.
     *
     * **Not `/releases/latest`.** Measured on 2026-09-20: this repository's
     * Releases page already holds `v1.0.0` and `v0.1.0`, both of which are OS
     * netinstall ISOs, and `v1.0.0` is what `/releases/latest` answers. An
     * updater built on that endpoint would fetch a release with no APK in it
     * and conclude, every single time, that there was nothing to update to —
     * quietly, forever. The APK and the ISO share one Releases page, so the
     * tag is what tells them apart.
     *
     * Drafts and prereleases are skipped: a draft is not visible to a phone
     * anyway, and a prerelease is by definition not what a stranger's phone
     * should install itself.
     */
    fun newestAndroidRelease(listingJson: String): GithubRelease? {
        val releases = try {
            json.decodeFromString<List<GithubRelease>>(listingJson)
        } catch (e: Exception) {
            // A listing that did not parse is an absence, not a crash: GitHub
            // answers rate-limiting with a JSON *object* carrying `message`,
            // and a captive portal answers with HTML. Neither is a reason to
            // show the owner of this phone an error.
            return null
        }
        return releases
            .filter { !it.draft && !it.prerelease }
            .filter { ANDROID_TAG.matches(it.tag) }
            .maxByOrNull { codeOfTag(it.tag) ?: 0 }
    }

    /** The versionCode inside an `android-v<N>` tag, or `null`. */
    fun codeOfTag(tag: String): Int? =
        ANDROID_TAG.matchEntire(tag)?.groupValues?.get(1)?.toIntOrNull()

    /**
     * The URL of the release's metadata asset — the `.json` written beside the
     * APK by `android/tools/release-artifacts.sh`.
     *
     * The app cannot construct any of these URLs: every asset name carries the
     * version, which is the thing being discovered. So the listing is the only
     * way in, and the metadata is read rather than guessed.
     */
    fun metadataUrl(release: GithubRelease): String? =
        release.assets.firstOrNull { it.name.endsWith(".json") }?.url

    /** Parse the metadata file, or `null` if it is not one. */
    fun parseMetadata(text: String): ReleaseMetadata? = try {
        json.decodeFromString<ReleaseMetadata>(text)
    } catch (e: Exception) {
        null
    }

    /**
     * What to offer the user, or `null` for "nothing".
     *
     * `null` covers every ordinary case: this build is current, this build is
     * NEWER than the release (which happens to whoever built the app
     * themselves, and must not offer them a downgrade Android would refuse
     * anyway), or the release does not actually contain the APK its own
     * metadata names.
     *
     * A release is only offered when its metadata is internally consistent
     * with the release it came from. The tag says a versionCode and so does
     * the metadata; if they disagree, something built that release wrongly and
     * the safe reading is to offer nothing rather than to pick a winner.
     */
    fun offer(installedCode: Int, release: GithubRelease, meta: ReleaseMetadata): UpdateOffer? {
        val tagCode = codeOfTag(release.tag) ?: return null
        if (tagCode != meta.versionCode) return null
        if (meta.versionCode <= installedCode) return null
        // Bounds before anything is fetched. A `sizeBytes` of zero, or one
        // absurdly large, means the metadata is wrong; streaming it and
        // finding out afterwards would have cost the user their data.
        if (meta.sizeBytes !in 1..MAX_APK_BYTES) return null
        if (!SHA256_HEX.matches(meta.sha256)) return null
        val apk = release.assets.firstOrNull { it.name == meta.apk } ?: return null
        return UpdateOffer(
            versionCode = meta.versionCode,
            versionName = meta.versionName,
            apkUrl = apk.url,
            sizeBytes = meta.sizeBytes,
            sha256 = meta.sha256,
            pageUrl = release.pageUrl,
        )
    }

    /**
     * Copy [from] into [into], and refuse to let a single byte that is not the
     * release's APK reach the installer.
     *
     * The digest is computed **as the bytes go past**, not afterwards from a
     * file, and the caller is expected to hand the installer session's own
     * stream in as [into] and to abandon that session if this throws. That
     * ordering is the whole point: verify-then-commit, never
     * commit-then-verify.
     *
     * It also means the APK never becomes a file this app owns — no download
     * directory, no `FileProvider`, nothing for `InsecureStorageTest`'s walk to
     * account for and nothing for `no-second-write-path.sh` to find. The bytes
     * go from the socket into a session the system owns.
     *
     * The length is enforced while copying rather than checked at the end, so
     * a server that streams forever cannot fill the phone before anyone
     * notices.
     */
    fun copyVerified(from: InputStream, into: OutputStream, expect: UpdateOffer) {
        val digest = SHA256Digest()
        val buffer = ByteArray(64 * 1024)
        var seen = 0L
        while (true) {
            val n = from.read(buffer)
            if (n < 0) break
            if (n == 0) continue
            seen += n
            if (seen > expect.sizeBytes) {
                throw UpdateRejected(
                    "the download is longer than the ${expect.sizeBytes} bytes the release " +
                        "declared, so it is not the file that was published",
                )
            }
            digest.update(buffer, 0, n)
            into.write(buffer, 0, n)
        }
        if (seen != expect.sizeBytes) {
            throw UpdateRejected(
                "the download stopped after $seen of ${expect.sizeBytes} bytes. Nothing was " +
                    "installed; the app is unchanged.",
            )
        }
        val out = ByteArray(digest.digestSize)
        digest.doFinal(out, 0)
        val got = hex(out)
        if (!constantTimeEquals(got, expect.sha256)) {
            throw UpdateRejected(
                "the downloaded file does not match the checksum the release published. " +
                    "Nothing was installed. Expected ${expect.sha256}, got $got.",
            )
        }
    }

    /**
     * An Android versionCode ceiling expressed in bytes: 512 MiB.
     *
     * Not a performance number. It is the point past which a `sizeBytes` is
     * more likely to be wrong — or hostile — than to be an APK, and having it
     * here means the refusal happens before the download rather than after it.
     */
    internal const val MAX_APK_BYTES: Long = 512L * 1024 * 1024

    private val SHA256_HEX = Regex("""^[0-9a-f]{64}$""")

    private fun hex(bytes: ByteArray): String {
        val sb = StringBuilder(bytes.size * 2)
        for (b in bytes) {
            val v = b.toInt() and 0xff
            sb.append("0123456789abcdef"[v ushr 4])
            sb.append("0123456789abcdef"[v and 0x0f])
        }
        return sb.toString()
    }

    /**
     * Compared without an early exit. There is no secret in a public checksum,
     * so this is not defence — it is the house rule applied uniformly, because
     * a comparison that is sometimes constant-time and sometimes not is a
     * comparison nobody can reason about at a glance.
     */
    private fun constantTimeEquals(a: String, b: String): Boolean {
        if (a.length != b.length) return false
        var diff = 0
        for (i in a.indices) diff = diff or (a[i].code xor b[i].code)
        return diff == 0
    }
}

/** One release as the GitHub API describes it — the four fields this app uses. */
@Serializable
data class GithubRelease(
    @SerialName("tag_name") val tag: String,
    @SerialName("html_url") val pageUrl: String = "",
    val draft: Boolean = false,
    val prerelease: Boolean = false,
    val assets: List<GithubAsset> = emptyList(),
)

/** One file attached to a release. */
@Serializable
data class GithubAsset(
    val name: String,
    @SerialName("browser_download_url") val url: String,
)

/**
 * The metadata file written beside the APK by `release-artifacts.sh`.
 *
 * It is a contract between that script and this object, and the fields with
 * defaults are the ones this app can do without — so that a build produced
 * before a field existed still parses. `versionCode`, `apk`, `sha256` and
 * `sizeBytes` have no defaults, because an update offered without any one of
 * them could not be verified.
 */
@Serializable
data class ReleaseMetadata(
    val versionCode: Int,
    val versionName: String = "",
    val tag: String = "",
    val commit: String = "",
    val apk: String,
    val sha256: String,
    val sizeBytes: Long,
    val remoteProtocolPreferred: Int = 0,
    val remoteProtocolSupported: List<Int> = emptyList(),
)

/** A newer build, and everything needed to fetch and check it. */
data class UpdateOffer(
    val versionCode: Int,
    val versionName: String,
    val apkUrl: String,
    val sizeBytes: Long,
    val sha256: String,
    val pageUrl: String,
)

/**
 * The downloaded bytes were not the published ones.
 *
 * This is the one failure in this file that is NOT quiet. Everything else that
 * goes wrong — no network, a captive portal, a rate limit, no release yet — is
 * an absence and the app carries on without a word. This one means the file
 * that arrived is not the file the release describes, and the user is told,
 * because the alternative is an app that silently stops updating and nobody
 * ever finds out why.
 */
class UpdateRejected(message: String) : Exception(message)
