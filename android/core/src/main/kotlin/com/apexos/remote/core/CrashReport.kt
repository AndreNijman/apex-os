package com.apexos.remote.core

/**
 * A crash report that cannot carry what this app handles (P1-060).
 *
 * > Crash reporting is consent-based and redacts sensitive terminal/task
 * > content.
 *
 * ## No third-party SDK, and that is the whole first half
 *
 * Crashlytics, Sentry and every relative of theirs work by uploading whatever
 * they can reach, to a service the user did not choose, from a process holding
 * a paired device key and a live terminal. Installing one and then configuring
 * it to behave is the wrong order: the default would be an upload, and a
 * default is what ships when somebody forgets a flag.
 *
 * So there is no reporter. A crash produces text, on the phone, that the user
 * can read in full and send if they want to. Nothing leaves the device unless
 * a person taps a button, and nothing is even WRITTEN unless consent is on —
 * which is the stronger placement, because a file that was never created
 * cannot be read by whatever gets at this phone next.
 *
 * ## Exception MESSAGES are dropped, and this is the rule that does the work
 *
 * A stack frame is a code location: a class, a method, a line. Those are
 * already in the binary and say nothing about what was being worked on.
 *
 * A message is data. `AgentError(kind, message)` carries the daemon's own
 * sentence, which quotes session ids and paths. A `SerializationException`
 * quotes the fragment it choked on, which here is the payload of a session
 * list — including [com.apexos.remote.core.agent.AgentSession.detail], the
 * field the daemon fills with Bash command lines, grep patterns, fetched URLs
 * and the agent's own notification text. An `IOException` from a socket names
 * the host and port of the machine. An `IllegalArgumentException` from a
 * parser routinely repeats its whole input.
 *
 * There is no way to tell a safe message from a dangerous one by looking at
 * it, and a redactor that tried would be a list of patterns somebody has to
 * keep ahead of every library in the dependency tree. So the type is kept and
 * the message is dropped, always, with no exceptions and no allowlist. What
 * that costs is real — a bug report that says `IllegalStateException` and not
 * why — and it is the right trade for an app that is a window onto somebody's
 * machine.
 *
 * ## What a report contains
 *
 * The build, the Android version, the device model, and for each exception in
 * the cause chain: its type, and its frames as `class.method(line)`. That is
 * enough to find the line that threw and to know which build did it.
 */
object CrashReport {

    /** Facts about the build and the phone. None of them is about a machine. */
    data class Environment(
        /** This app's `versionName`. */
        val appVersion: String,
        /** `Build.VERSION.RELEASE`. */
        val androidRelease: String,
        /** `Build.VERSION.SDK_INT`. */
        val sdkInt: Int,
        /**
         * `Build.MODEL`.
         *
         * Reduced to the same alphabet a file name is, and truncated. A model
         * string comes from the device's own build properties, and on a phone
         * whose ROM somebody else built it is whatever they typed there.
         */
        val model: String,
    )

    /** Longest report written. A deep recursion produces thousands of frames. */
    const val MAX_FRAMES: Int = 60

    /** Longest cause chain followed, before it is called a loop. */
    const val MAX_CAUSES: Int = 8

    /**
     * Characters a value taken from the device may contain.
     *
     * Same reasoning as `apex_agent_core::inject::SAFE`: the value ends up in
     * a file the user may share, and a name with a newline in it is a value
     * that can forge a line of the report.
     */
    private const val SAFE = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789._- "

    private fun safe(raw: String, max: Int = 48): String =
        raw.take(max).map { if (it.isAscii() && SAFE.contains(it)) it else '_' }.joinToString("")

    private fun Char.isAscii(): Boolean = code < 128

    /**
     * Whether a thread name may be printed.
     *
     * Thread names are usually constants, and "usually" is not a property to
     * rest on: a pool that names its workers after what they are doing would
     * put that in the report. Only names that look like a library's own
     * constant survive; anything else becomes a description.
     */
    private val PLAIN_THREAD = Regex("""[A-Za-z][A-Za-z0-9 _.-]{0,39}""")

    /**
     * The report for one crash.
     *
     * @param thread the name of the thread that died.
     */
    fun of(t: Throwable, env: Environment, thread: String): String = buildString {
        appendLine("APEX Remote crash report")
        appendLine("app ${safe(env.appVersion, 24)}")
        appendLine("android ${safe(env.androidRelease, 16)} (sdk ${env.sdkInt})")
        appendLine("device ${safe(env.model)}")
        appendLine(
            "thread " + if (PLAIN_THREAD.matches(thread)) thread else "(a name this report will not print)",
        )
        appendLine()
        appendLine(
            "No exception messages appear below. A message is data — the daemon's own " +
                "sentence, a parser's input, a socket's host — and there is no way to tell a " +
                "safe one from a dangerous one, so every one of them is dropped.",
        )

        var current: Throwable? = t
        var depth = 0
        val seen = mutableSetOf<Throwable>()
        while (current != null && depth < MAX_CAUSES && seen.add(current)) {
            appendLine()
            appendLine(if (depth == 0) "thrown: ${current.javaClass.name}" else "caused by: ${current.javaClass.name}")
            val frames = current.stackTrace
            for (f in frames.take(MAX_FRAMES)) {
                appendLine("  at ${f.className}.${f.methodName}(${f.lineNumber})")
            }
            if (frames.size > MAX_FRAMES) {
                appendLine("  ... ${frames.size - MAX_FRAMES} more frames")
            }
            current = current.cause
            depth++
        }
        if (current != null) {
            // Either deeper than MAX_CAUSES or a cycle. Both are said, because
            // "the report stops here" is a different fact from "that was all".
            appendLine()
            appendLine("(the cause chain goes on, or loops; it is not followed further)")
        }
    }

    /**
     * Whether a finished report is safe to keep.
     *
     * Belt over braces, and it earns its place: [of] is the only writer today,
     * and the next person to add a line to it will not re-read this file. A
     * report that somehow carries a newline-forged line, or grows past what a
     * crash can plausibly need, is dropped rather than stored.
     *
     * It does NOT try to detect secrets by pattern. There is no such test, and
     * one that looked for `password` would pass a report full of session
     * output and feel as though it had checked.
     */
    fun isWellFormed(report: String): Boolean =
        report.length <= 64_000 &&
            report.startsWith("APEX Remote crash report") &&
            report.lineSequence().all { line ->
                line.isEmpty() ||
                    line.startsWith("  at ") ||
                    line.startsWith("  ... ") ||
                    line.startsWith("APEX Remote crash report") ||
                    line.startsWith("app ") ||
                    line.startsWith("android ") ||
                    line.startsWith("device ") ||
                    line.startsWith("thread ") ||
                    line.startsWith("thrown: ") ||
                    line.startsWith("caused by: ") ||
                    line.startsWith("No exception messages") ||
                    line.startsWith("safe one from") ||
                    line.startsWith("(the cause chain")
            }

    /**
     * What the consent setting says, in the words the user is shown.
     *
     * In `:core` so a test can assert the app never offers something the
     * report cannot honour. It says what is kept and what is dropped, because
     * consent to "crash reporting" with no statement of contents is not
     * consent to anything in particular.
     */
    const val CONSENT_EXPLANATION: String =
        "Off by default. With this on, a crash writes a report to this phone and nowhere " +
            "else: the build, the Android version, the device model, and the type and code " +
            "lines of what failed. Error messages are never included, because they carry " +
            "whatever the app was handling — a path, a command an agent ran, part of a " +
            "session. Nothing is sent anywhere; you read the report and choose whether to " +
            "share it. With this off, nothing is written at all."
}
