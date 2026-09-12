package com.apexos.remote.core

import com.apexos.remote.core.agent.AgentSession
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.io.TempDir
import java.io.File
import java.io.IOException

/**
 * P1-060's fourth criterion: consent-based, and redacts terminal/task content.
 *
 * The assertions that matter are driven through the WHOLE path — a real
 * exception carrying real secrets, through [CrashReport.of], through
 * [AppStorage.saveCrash], and then out of the file on disk. Asserting against
 * a hand-built report would be vacuous in the same way `NotificationsTest`
 * describes: a report with nothing in it cannot leak anything.
 */
class CrashReportTest {

    private val env = CrashReport.Environment(
        appVersion = "0.1.0",
        androidRelease = "16",
        sdkInt = 36,
        model = "Pixel 8",
    )

    /**
     * The things that must never appear, and where each really comes from.
     *
     * Not invented strings. Each is the kind of value a real exception message
     * in this app carries, which is the reason messages are dropped wholesale
     * rather than filtered.
     */
    private val secrets = listOf(
        // SessionInfo.detail — hook.rs copies Bash command lines verbatim.
        "curl -H 'Authorization: Bearer sk-live-9f2c' https://api.internal",
        // A path on the machine, which an IOException from a socket or a file
        // names as a matter of course.
        "/home/andre/Projects/apex/.env",
        // The user's own words, which is what a reply or a dictated sentence is.
        "delete the staging database",
        // A pairing token, which a parse error over a pairing payload quotes.
        "apex-remote://pair?k=7QnR3vTgH1sXbZ",
        // The machine's address.
        "192.168.1.245:7719",
    )

    private fun poisoned(): Throwable {
        val root = IOException(
            "failed to read ${secrets[1]} while connecting to ${secrets[4]}",
        )
        val middle = IllegalStateException(
            "could not parse session: detail=${secrets[0]}",
            root,
        )
        return RuntimeException("${secrets[2]} / ${secrets[3]}", middle)
    }

    @Test
    fun `not one exception message survives into the report`() {
        val report = CrashReport.of(poisoned(), env, thread = "main")
        for (s in secrets) {
            assertFalse(
                report.contains(s),
                "the report carries `$s`, which came out of an exception message",
            )
        }
        // And none of the messages' own framing either, which is how a
        // half-redaction usually shows up.
        for (fragment in listOf("failed to read", "could not parse session", "detail=")) {
            assertFalse(report.contains(fragment), "a message fragment survived: $fragment")
        }
    }

    @Test
    fun `the types and code lines DO survive, or the report is useless`() {
        // The other half. A redactor that dropped everything would pass the
        // test above and be worth nothing, which is the failure mode a leak
        // test invites.
        val report = CrashReport.of(poisoned(), env, thread = "main")
        assertTrue(report.contains("java.lang.RuntimeException"), report)
        assertTrue(report.contains("java.lang.IllegalStateException"), report)
        assertTrue(report.contains("java.io.IOException"), report)
        assertTrue(report.contains("caused by:"), report)
        assertTrue(
            report.contains("com.apexos.remote.core.CrashReportTest.poisoned"),
            "no frame from the code that threw: $report",
        )
        assertTrue(report.contains("0.1.0") && report.contains("sdk 36"), report)
    }

    @Test
    fun `a session record's detail cannot reach a report through any path`() {
        // Driven from the type the daemon actually sends, so this keeps
        // holding if somebody starts putting a session into a message.
        val session = AgentSession(
            id = 4,
            agent = "claude",
            detail = secrets[0],
            project = secrets[1],
            cwd = secrets[1],
        )
        val report = CrashReport.of(
            IllegalArgumentException("bad session: $session"),
            env,
            thread = "main",
        )
        assertFalse(report.contains(secrets[0]))
        assertFalse(report.contains(secrets[1]))
        assertFalse(report.contains("claude"), "the report named the agent: $report")
    }

    @Test
    fun `a thread named after what it was doing is not printed`() {
        // Thread names are usually constants. "Usually" is not a property to
        // rest on when the alternative is a pool that names workers after
        // their work.
        val plain = CrashReport.of(RuntimeException(), env, thread = "DefaultDispatcher-worker-3")
        assertTrue(plain.contains("DefaultDispatcher-worker-3"))
        val loud = CrashReport.of(
            RuntimeException(),
            env,
            thread = "session-4 /home/andre/secret",
        )
        assertFalse(loud.contains("/home/andre/secret"), loud)
        assertTrue(loud.contains("will not print"), loud)
    }

    @Test
    fun `a device model somebody else typed cannot forge a line`() {
        // Build.MODEL comes from the device's own build properties, which on a
        // phone running somebody else's ROM is whatever they put there.
        val hostile = CrashReport.of(
            RuntimeException(),
            env.copy(model = "Pixel\nthrown: java.lang.Nothing\nfake"),
            thread = "main",
        )
        assertEquals(
            1,
            hostile.lines().count { it.startsWith("thrown: ") },
            "a device model forged a second `thrown:` line: $hostile",
        )
        assertTrue(CrashReport.isWellFormed(hostile))
    }

    @Test
    fun `a cause chain that loops is stopped and said to be stopped`() {
        // A truncated report and a complete one are different facts, and a
        // reader who cannot tell them apart chases a cause that was never
        // there.
        val a = RuntimeException("a")
        val b = IllegalStateException("b", a)
        a.initCause(b)
        val report = CrashReport.of(a, env, thread = "main")
        assertTrue(report.contains("goes on, or loops"), report)
        assertTrue(CrashReport.isWellFormed(report))
    }

    @Test
    fun `a deep stack is cut and the cut is counted`() {
        val deep = RuntimeException()
        deep.stackTrace = Array(CrashReport.MAX_FRAMES + 25) {
            StackTraceElement("com.apexos.Deep", "go", "Deep.kt", it)
        }
        val report = CrashReport.of(deep, env, thread = "main")
        assertEquals(CrashReport.MAX_FRAMES, report.lines().count { it.startsWith("  at ") })
        assertTrue(report.contains("... 25 more frames"), report)
    }

    // ── consent gates the WRITE, not a send ─────────────────────────────────

    @Test
    fun `with consent off, nothing is written at all`(@TempDir dir: File) {
        val storage = AppStorage(dir)
        val report = CrashReport.of(poisoned(), env, thread = "main")
        assertFalse(storage.saveCrash(report, consented = false))
        assertFalse(storage.crashFile.exists(), "a report was written without consent")
        assertNull(storage.loadCrash())
        // The whole directory, not just the file this test knows the name of.
        assertTrue(
            dir.walkTopDown().filter { it.isFile }.toList().isEmpty(),
            "something was written: ${dir.walkTopDown().filter { it.isFile }.toList()}",
        )
    }

    @Test
    fun `consent defaults to off`() {
        // The criterion is "consent-based". A switch that starts on and can be
        // found is not that.
        assertFalse(Settings().crashReports)
        assertFalse(MachineStore().settings.crashReports)
        // And a store written before the field existed reads as off rather
        // than as on, which is what an absent key must mean here.
        assertFalse(MachineStore.decode("""{"v":1,"machines":[],"settings":{}}""").settings.crashReports)
    }

    @Test
    fun `with consent on, the file on disk holds no secret either`(@TempDir dir: File) {
        // The assertion made against the BYTES, not against the string that
        // was handed to the writer. A test that only checked the return value
        // of `of` would miss an encoder, a wrapper, or a second file.
        val storage = AppStorage(dir)
        assertTrue(storage.saveCrash(CrashReport.of(poisoned(), env, "main"), consented = true))
        val files = dir.walkTopDown().filter { it.isFile }.toList()
        assertEquals(listOf(storage.crashFile), files, "an unexpected file appeared: $files")
        val bytes = storage.crashFile.readBytes()
        for (s in secrets) {
            for (encoding in listOf(Charsets.UTF_8, Charsets.UTF_16LE, Charsets.ISO_8859_1)) {
                assertFalse(
                    indexOf(bytes, s.toByteArray(encoding)) >= 0,
                    "`$s` is in $encoding in the file on disk",
                )
            }
        }
        assertTrue(storage.loadCrash()!!.contains("java.io.IOException"))
    }

    @Test
    fun `a malformed report is refused even with consent`(@TempDir dir: File) {
        // The consent check and the shape check are both in `saveCrash`, and
        // not at the call site: the call site is an uncaught-exception handler,
        // which is the least exercised code in any app.
        val storage = AppStorage(dir)
        assertFalse(storage.saveCrash("whatever happened", consented = true))
        assertFalse(storage.saveCrash("x".repeat(70_000), consented = true))
        assertFalse(storage.crashFile.exists())
    }

    @Test
    fun `clearing a report removes it`(@TempDir dir: File) {
        val storage = AppStorage(dir)
        storage.saveCrash(CrashReport.of(RuntimeException(), env, "main"), consented = true)
        assertTrue(storage.crashFile.exists())
        storage.clearCrash()
        assertFalse(storage.crashFile.exists())
        assertNull(storage.loadCrash())
    }

    @Test
    fun `the consent wording says what is kept and what is dropped`() {
        // Consent to "crash reporting" with no statement of contents is not
        // consent to anything in particular.
        val t = CrashReport.CONSENT_EXPLANATION
        assertTrue(t.contains("Off by default"), t)
        assertTrue(t.contains("never included"), t)
        assertTrue(t.contains("Nothing is sent anywhere"), t)
        assertTrue(t.contains("nothing is written at all"), t)
    }

    private fun indexOf(haystack: ByteArray, needle: ByteArray): Int {
        if (needle.isEmpty() || needle.size > haystack.size) return -1
        outer@ for (i in 0..haystack.size - needle.size) {
            for (j in needle.indices) if (haystack[i + j] != needle[j]) continue@outer
            return i
        }
        return -1
    }
}
