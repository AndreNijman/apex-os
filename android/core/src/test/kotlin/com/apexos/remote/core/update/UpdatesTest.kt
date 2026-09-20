package com.apexos.remote.core.update

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertNotNull
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.assertThrows
import org.bouncycastle.crypto.digests.SHA256Digest
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream

/**
 * The updater, exercised against the two things that actually break it: a
 * Releases page that is not only Android releases, and a download that is not
 * the file the release described.
 *
 * Neither is hypothetical. The first was MEASURED on 2026-09-20 —
 * `AndreNijman/apex-os` already publishes OS netinstall ISOs to the same
 * Releases page, and `/releases/latest` answers `v1.0.0`, an ISO release with
 * no APK in it. An updater written the obvious way would have looked there,
 * found nothing, and reported "up to date" to every phone forever.
 */
class UpdatesTest {

    // ── Fixtures ─────────────────────────────────────────────────────────────

    private fun sha256Of(bytes: ByteArray): String {
        val d = SHA256Digest()
        d.update(bytes, 0, bytes.size)
        val out = ByteArray(d.digestSize)
        d.doFinal(out, 0)
        return out.joinToString("") { "%02x".format(it) }
    }

    /**
     * The real shape of this repository's Releases page: two OS ISO releases
     * and one Android release, newest ISO first — which is the order GitHub
     * answers in and the reason "take the first one" is wrong.
     */
    private val mixedListing = """
        [
          {
            "tag_name": "v1.0.0",
            "html_url": "https://github.com/AndreNijman/apex-os/releases/tag/v1.0.0",
            "draft": false, "prerelease": false,
            "assets": [
              {"name": "apex-os-netinstall-x86_64.iso",
               "browser_download_url": "https://example.invalid/iso"},
              {"name": "apex-os-netinstall-x86_64.iso.sha256",
               "browser_download_url": "https://example.invalid/iso.sha256"}
            ]
          },
          {
            "tag_name": "android-v1306",
            "html_url": "https://github.com/AndreNijman/apex-os/releases/tag/android-v1306",
            "draft": false, "prerelease": false,
            "assets": [
              {"name": "apex-remote-0.1.0+1306.gdeadbeef.apk",
               "browser_download_url": "https://example.invalid/apk"},
              {"name": "apex-remote-0.1.0+1306.gdeadbeef.apk.sha256",
               "browser_download_url": "https://example.invalid/apk.sha256"},
              {"name": "apex-remote-0.1.0+1306.gdeadbeef.json",
               "browser_download_url": "https://example.invalid/meta"}
            ]
          },
          {
            "tag_name": "v0.1.0", "html_url": "", "draft": false, "prerelease": false,
            "assets": [{"name": "apex-os-daily-netinstall.iso",
                        "browser_download_url": "https://example.invalid/old-iso"}]
          }
        ]
    """.trimIndent()

    private fun metadataJson(
        code: Int = 1306,
        apk: String = "apex-remote-0.1.0+1306.gdeadbeef.apk",
        sha: String = "a".repeat(64),
        size: Long = 12_345,
        tag: String = "android-v1306",
    ) = """
        {
          "versionCode": $code,
          "versionName": "0.1.0+$code.gdeadbeef",
          "tag": "$tag",
          "commit": "303221d5",
          "apk": "$apk",
          "sha256": "$sha",
          "sizeBytes": $size,
          "remoteProtocolPreferred": 1,
          "remoteProtocolSupported": [1]
        }
    """.trimIndent()

    // ── Finding the release ──────────────────────────────────────────────────

    @Test
    fun `the android release is found on a page that is mostly OS images`() {
        val found = Updates.newestAndroidRelease(mixedListing)
        assertNotNull(found)
        assertEquals("android-v1306", found!!.tag)
        // Said explicitly: the ISO release is BOTH newer in the listing order
        // and what `/releases/latest` would have returned. Picking by tag is
        // the whole mechanism.
        assertTrue(mixedListing.indexOf("v1.0.0") < mixedListing.indexOf("android-v1306"))
    }

    @Test
    fun `a page with no android release at all offers nothing, quietly`() {
        val isosOnly = """
            [{"tag_name": "v1.0.0", "html_url": "", "draft": false, "prerelease": false,
              "assets": [{"name": "apex-os-netinstall-x86_64.iso",
                          "browser_download_url": "https://example.invalid/iso"}]}]
        """.trimIndent()
        assertNull(Updates.newestAndroidRelease(isosOnly))
    }

    @Test
    fun `the newest android release wins, not the first in the listing`() {
        val listing = """
            [{"tag_name": "android-v900", "html_url": "", "draft": false, "prerelease": false,
              "assets": []},
             {"tag_name": "android-v1306", "html_url": "", "draft": false, "prerelease": false,
              "assets": []}]
        """.trimIndent()
        assertEquals("android-v1306", Updates.newestAndroidRelease(listing)?.tag)
    }

    @Test
    fun `drafts and prereleases are not offered to a stranger's phone`() {
        val listing = """
            [{"tag_name": "android-v2000", "html_url": "", "draft": true, "prerelease": false,
              "assets": []},
             {"tag_name": "android-v1900", "html_url": "", "draft": false, "prerelease": true,
              "assets": []},
             {"tag_name": "android-v1306", "html_url": "", "draft": false, "prerelease": false,
              "assets": []}]
        """.trimIndent()
        assertEquals("android-v1306", Updates.newestAndroidRelease(listing)?.tag)
    }

    @Test
    fun `a rate limit, a captive portal and plain rubbish are all absences`() {
        // GitHub answers a rate limit with a JSON OBJECT, not a list; a captive
        // portal answers with HTML. Neither may throw, because both happen to
        // an ordinary phone on an ordinary day and neither is this app's fault.
        assertNull(Updates.newestAndroidRelease("""{"message":"API rate limit exceeded"}"""))
        assertNull(Updates.newestAndroidRelease("<html><body>Sign in to the Wi-Fi</body></html>"))
        assertNull(Updates.newestAndroidRelease(""))
        assertNull(Updates.parseMetadata("not json at all"))
        assertNull(Updates.parseMetadata("""{"versionName":"0.1.0"}"""))
    }

    @Test
    fun `the metadata asset is found by extension, because its name carries the version`() {
        val release = Updates.newestAndroidRelease(mixedListing)!!
        assertEquals("https://example.invalid/meta", Updates.metadataUrl(release))
    }

    // ── Deciding ─────────────────────────────────────────────────────────────

    private fun offerFor(installed: Int, metaText: String = metadataJson()): UpdateOffer? {
        val release = Updates.newestAndroidRelease(mixedListing)!!
        val meta = Updates.parseMetadata(metaText) ?: return null
        return Updates.offer(installed, release, meta)
    }

    @Test
    fun `a newer release is offered, with the URL of the APK the metadata names`() {
        val offer = offerFor(installed = 1300)
        assertNotNull(offer)
        assertEquals(1306, offer!!.versionCode)
        assertEquals("https://example.invalid/apk", offer.apkUrl)
        assertEquals(12_345L, offer.sizeBytes)
    }

    @Test
    fun `the same build is not offered to itself`() {
        assertNull(offerFor(installed = 1306))
    }

    @Test
    fun `a build NEWER than the release is not offered a downgrade`() {
        // Whoever builds this app themselves runs ahead of the release. Android
        // would refuse the install anyway; offering it would be a prompt that
        // can only ever fail.
        assertNull(offerFor(installed = 9000))
    }

    @Test
    fun `a release whose tag and metadata disagree is not offered`() {
        // Something built that release wrongly. Picking a winner between two
        // version numbers is how a phone ends up installing something nobody
        // meant to publish.
        assertNull(offerFor(installed = 1, metaText = metadataJson(code = 1307)))
    }

    @Test
    fun `a release whose metadata names an APK that is not attached is not offered`() {
        assertNull(offerFor(installed = 1, metaText = metadataJson(apk = "apex-remote-other.apk")))
    }

    @Test
    fun `a metadata file with an unusable size or checksum is not offered`() {
        assertNull(offerFor(installed = 1, metaText = metadataJson(size = 0)))
        assertNull(offerFor(installed = 1, metaText = metadataJson(size = Updates.MAX_APK_BYTES + 1)))
        assertNull(offerFor(installed = 1, metaText = metadataJson(sha = "not a digest")))
        assertNull(offerFor(installed = 1, metaText = metadataJson(sha = "A".repeat(64))))
    }

    @Test
    fun `a metadata file from a future release still parses`() {
        // The release flow will grow fields. A strict parser would turn that
        // into "every installed phone stopped seeing updates", which is the
        // same drift failure one level up from the protocol window.
        val future = metadataJson().dropLast(1) + """, "signedBy": "sigstore", "channel": "stable"}"""
        assertNotNull(Updates.parseMetadata(future))
    }

    // ── Verifying the download ───────────────────────────────────────────────

    private fun offerOf(bytes: ByteArray, sha: String = sha256Of(bytes)) = UpdateOffer(
        versionCode = 1306,
        versionName = "0.1.0+1306.gdeadbeef",
        apkUrl = "https://example.invalid/apk",
        sizeBytes = bytes.size.toLong(),
        sha256 = sha,
        pageUrl = "",
    )

    @Test
    fun `the right bytes are copied through and the digest agrees`() {
        val apk = ByteArray(200_000) { (it * 31).toByte() }
        val sink = ByteArrayOutputStream()
        Updates.copyVerified(ByteArrayInputStream(apk), sink, offerOf(apk))
        assertTrue(apk.contentEquals(sink.toByteArray()), "the bytes did not arrive intact")
    }

    @Test
    fun `one flipped byte is refused`() {
        val apk = ByteArray(4096) { it.toByte() }
        val expect = offerOf(apk)
        val tampered = apk.copyOf().also { it[2000] = (it[2000] + 1).toByte() }
        val rejected = assertThrows<UpdateRejected> {
            Updates.copyVerified(ByteArrayInputStream(tampered), ByteArrayOutputStream(), expect)
        }
        assertTrue(rejected.message!!.contains("does not match the checksum"), rejected.message!!)
        // The one failure in this file that is NOT quiet: the user is told,
        // and told that nothing was installed.
        assertTrue(rejected.message!!.contains("Nothing was installed"), rejected.message!!)
    }

    @Test
    fun `a download that stops short is refused, and says how far it got`() {
        val apk = ByteArray(4096) { it.toByte() }
        val expect = offerOf(apk)
        val rejected = assertThrows<UpdateRejected> {
            Updates.copyVerified(ByteArrayInputStream(apk.copyOf(1000)), ByteArrayOutputStream(), expect)
        }
        assertTrue(rejected.message!!.contains("1000 of 4096"), rejected.message!!)
    }

    @Test
    fun `a server that streams forever is cut off at the declared length`() {
        // Checked WHILE copying rather than at the end. A stream with no end
        // would otherwise fill the phone before anyone noticed, and "the
        // download failed" is a much better outcome than a full disk.
        val endless = object : InputStream() {
            override fun read(): Int = 0
            override fun read(b: ByteArray, off: Int, len: Int): Int {
                java.util.Arrays.fill(b, off, off + len, 0)
                return len
            }
        }
        val written = object : OutputStream() {
            var count = 0L
            override fun write(b: Int) { count++ }
            override fun write(b: ByteArray, off: Int, len: Int) { count += len }
        }
        val expect = offerOf(ByteArray(0), sha = "b".repeat(64)).copy(sizeBytes = 100_000)
        val rejected = assertThrows<UpdateRejected> { Updates.copyVerified(endless, written, expect) }
        assertTrue(rejected.message!!.contains("longer than"), rejected.message!!)
        // Bounded, and bounded tightly: one buffer's overshoot, not a gigabyte.
        assertTrue(written.count < 100_000 + 128 * 1024, "wrote ${written.count} bytes past the limit")
    }

    @Test
    fun `a socket that dies mid-download is an IO failure, not a rejection`() {
        // The distinction matters to the app: an IO failure is the phone
        // losing Wi-Fi in a lift and must be silent, a rejection is the file
        // being wrong and must be shown.
        val dying = object : InputStream() {
            var served = 0
            override fun read(): Int = throw IOException("connection reset")
            override fun read(b: ByteArray, off: Int, len: Int): Int {
                if (served++ > 0) throw IOException("connection reset")
                java.util.Arrays.fill(b, off, off + 16, 1)
                return 16
            }
        }
        assertThrows<IOException> {
            Updates.copyVerified(dying, ByteArrayOutputStream(), offerOf(ByteArray(0), "c".repeat(64)).copy(sizeBytes = 5_000))
        }
    }

    @Test
    fun `an empty download of an empty release is still refused`() {
        // Degenerate, and worth pinning: a zero-length APK whose digest happens
        // to be quoted correctly must not install. `offer` refuses size 0
        // before this is ever reached, and this is the second door.
        val expect = offerOf(ByteArray(0), sha = sha256Of(ByteArray(0))).copy(sizeBytes = 10)
        assertThrows<UpdateRejected> {
            Updates.copyVerified(ByteArrayInputStream(ByteArray(0)), ByteArrayOutputStream(), expect)
        }
    }
}
