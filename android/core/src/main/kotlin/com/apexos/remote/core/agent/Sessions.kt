package com.apexos.remote.core.agent

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonPrimitive

/**
 * `apex-agentd`'s `SessionInfo`, as much of it as a phone shows.
 *
 * ## The shapes that catch people
 *
 * * **`Response::Session` does not nest.** It is a serde newtype variant under
 *   an internal tag, so the session's fields sit *beside* `"reply"` rather
 *   than under a `"session"` key. `Response::Sessions` does nest, because
 *   serde cannot serialise a newtype variant wrapping a sequence at all.
 * * **Nothing is `skip_serializing_if`.** An absent optional arrives as
 *   `null`, and `children` arrives as `[]` rather than being missing — the
 *   daemon's own comment says the distinction is load-bearing: an empty list
 *   means "asked and found none", a missing key would mean "this daemon does
 *   not have the graph".
 * * **`started` and `last_activity` are unix SECONDS.** Everything else with a
 *   time in it — `grant_expires_ms`, `ttl_ms` — is milliseconds.
 * * The policy is flattened into the session, so `sandbox`, `network` and the
 *   rest are top-level keys. `origin` among them is policy dimension six;
 *   *which* origin asked is the separate `request_origin`, in kebab-case.
 */
@Serializable
data class AgentSession(
    val id: Int,
    /** The adapter: `claude`, `codex`, `generic`. Not the program. */
    val agent: String = "generic",
    val program: String = "",
    val args: List<String> = emptyList(),
    val cwd: String = "",
    val project: String? = null,
    @SerialName("project_name") val projectName: String? = null,
    val worktree: String? = null,
    val state: String = AgentStates.STARTING,
    /** Free text the runtime attached to the state. `"paused"`, an error, a step. */
    val detail: String? = null,
    val paused: Boolean = false,

    // The policy, flattened. Six of the seven dimensions; `connectors` is
    // omitted because nothing on a phone shows it.
    val sandbox: String? = null,
    val network: String? = null,
    val system: String? = null,
    val secrets: String? = null,
    val native: String? = null,

    @SerialName("request_origin") val requestOrigin: String? = null,
    @SerialName("origin_source") val originSource: String? = null,
    /** Which device asked, when a device did. This phone's own id, usually. */
    val actor: String? = null,

    val telemetry: Telemetry? = null,
    val children: List<ChildInfo> = emptyList(),

    val pid: Int = 0,
    /** Unix **seconds**. */
    val started: Long = 0,
    /** Unix **seconds** of the last PTY output or published event. */
    @SerialName("last_activity") val lastActivity: Long = 0,
    @SerialName("exit_code") val exitCode: Int? = null,
    @SerialName("exit_signal") val exitSignal: Int? = null,
    /** How many clients are attached, this phone included once it attaches. */
    val attached: Int = 0,
    val cols: Int = 80,
    val rows: Int = 24,
) {
    val tone: Tone get() = AgentStates.tone(state)
    val needsYou: Boolean get() = AgentStates.needsYou(state)
    val isTerminal: Boolean get() = AgentStates.isTerminal(state)
    val agentName: String get() = AgentNames.of(agent)

    /** Project name, project path, or working directory — the first that exists. */
    val where: String get() = projectName ?: project ?: cwd

    /**
     * How long this session has been going, in milliseconds.
     *
     * [nowMs] is passed in rather than read, because a list that recomputed
     * `System.currentTimeMillis()` per row would show rows a millisecond apart
     * and because a test cannot pin a clock it does not hold.
     */
    fun elapsedMs(nowMs: Long): Long = (nowMs - started * 1000L).coerceAtLeast(0)

    /**
     * How long since anything happened, in milliseconds.
     *
     * Worth showing separately from [elapsedMs] on anything not working: a
     * session that has been "waiting for you" for two minutes and one that has
     * been waiting for two hours are different situations.
     */
    fun idleMs(nowMs: Long): Long = (nowMs - lastActivity * 1000L).coerceAtLeast(0)
}

/**
 * What the agent told the runtime about itself.
 *
 * Every field is optional and `null` means **"we have never heard"**, which is
 * not the same as zero. A context gauge drawn at 0% for a session that has
 * never reported is a gauge that is lying.
 *
 * The two rate-limit figures are **account-wide, not per session**. Six
 * sessions on one login report the same number, and a list that drew it per
 * row would be repeating one fact six times; the daemon's own comment says so.
 */
@Serializable
data class Telemetry(
    /** `"Opus 4.5"`, not `claude-opus-4-5`. */
    val model: String? = null,
    @SerialName("context_pct") val contextPct: Double? = null,
    @SerialName("five_hour_pct") val fiveHourPct: Double? = null,
    @SerialName("five_hour_reset") val fiveHourReset: Long? = null,
    @SerialName("seven_day_pct") val sevenDayPct: Double? = null,
    @SerialName("seven_day_reset") val sevenDayReset: Long? = null,
    /** The git branch, read from `.git/HEAD` by the daemon, not from the agent. */
    val branch: String? = null,
    @SerialName("observed_at") val observedAt: Long = 0,
)

/**
 * One node of the agent graph: a subagent, or a process the session forked.
 *
 * The graph is **intra-session**. `parent` names another child *within the
 * same session*, which is what makes it a tree — an MCP server forked by a
 * language server forked by the agent is three levels down. There is no
 * parent-session field anywhere, and a phone that drew one would be inventing
 * a relationship the runtime does not model.
 */
@Serializable
data class ChildInfo(
    val id: String,
    /** `subagent` or `process`. */
    val kind: String = "process",
    val label: String = "",
    val started: Long = 0,
    val ended: Long? = null,
    @SerialName("ended_by") val endedBy: String? = null,
    val parent: String? = null,
    val pid: Int? = null,
    @SerialName("rss_kb") val rssKb: Long? = null,
) {
    val live: Boolean get() = ended == null
    val isSubagent: Boolean get() = kind == "subagent"
}

/** What the daemon says about itself, and the only request a mismatched client can rely on. */
@Serializable
data class Hello(
    val version: Int = 0,
    /**
     * The adapters this runtime has.
     *
     * This is the closest thing to the "profile" P1-054's criterion names.
     * There is no profile verb on this socket — `apex-agent-core/src/profile.rs`
     * exists and is not reachable through `Request` — so what a phone can
     * actually offer when starting an agent is: which adapter, which directory,
     * which worktree. Saying that plainly is better than inventing a picker
     * for something the runtime will not accept.
     */
    val agents: List<String> = emptyList(),
    @SerialName("default_agent") val defaultAgent: String = "",
)

/** The daemon said no, in its own vocabulary. */
class AgentError(val kind: String, override val message: String) : Exception(message)

/**
 * Building requests and reading replies.
 *
 * Requests are built as strings rather than serialised from objects, and that
 * is deliberate for one reason: a control payload **must not contain a
 * newline** — `apex-remote-core`'s wire refuses one in both directions,
 * because a payload carrying its own terminator would let a client smuggle a
 * second request into one frame. A hand-built line is one it is obvious to
 * check; a serialiser's pretty-printer is one it is easy to forget.
 */
object Agentd {
    val json: Json = Json {
        // A daemon newer than this app sends fields it has never heard of, and
        // the right response to that is to show what it does understand.
        ignoreUnknownKeys = true
        explicitNulls = false
        encodeDefaults = false
    }

    fun hello(): String = """{"cmd":"hello"}"""

    fun list(): String = """{"cmd":"list"}"""

    fun info(id: Int): String = """{"cmd":"info","id":$id}"""

    fun attach(id: Int, cols: Int, rows: Int, replay: Int = DEFAULT_REPLAY): String =
        """{"cmd":"attach","id":$id,"cols":$cols,"rows":$rows,"replay":$replay}"""

    fun resize(id: Int, cols: Int, rows: Int): String =
        """{"cmd":"resize","id":$id,"cols":$cols,"rows":$rows}"""

    /**
     * A signal, by the daemon's own names.
     *
     * `pause` and `resume` are not verbs on this socket; they are `SIGSTOP`
     * and `SIGCONT`, and `paused` becomes true only after the kill succeeds.
     * Naming them here rather than at the call site keeps that fact in one
     * place instead of in every button.
     */
    fun signal(id: Int, signal: String): String =
        """{"cmd":"signal","id":$id,"signal":"${escape(signal)}"}"""

    fun pause(id: Int): String = signal(id, "stop")

    fun resume(id: Int): String = signal(id, "cont")

    /** Ask politely. `kill` is `SIGKILL` and skips whatever cleanup the agent does. */
    fun stop(id: Int): String = signal(id, "term")

    fun interrupt(id: Int): String = signal(id, "int")

    /**
     * Per-worktree status for every remembered project, or for one slug.
     *
     * ## This verb EXISTS. The comment that used to stand here said it did not.
     *
     * An earlier round of this app built exactly this request, and then
     * deleted it along with its parser under the belief that
     * `apex-agent-core`'s `Request` vocabulary "is Hello, Run, List, Info,
     * Attach, Resize, Signal, Event, Logs, Remove, Prune, and the privilege
     * and secret verbs". That was read off `request.rs`'s `Verb` — the
     * *privileged operation* vocabulary — and not off `protocol.rs`'s
     * `Request`, which is the wire. `protocol.rs` has carried
     * `Worktrees { project: Option<String> }` since `473b7f60` (2026-09-08);
     * `apex-agentd/src/main.rs:906` dispatches it; and `apex-remoted`'s
     * `control()` refuses exactly one verb, `attach`, so it is forwarded.
     * See [WorktreeStatus] for the full account.
     *
     * `project` is a project **slug**, never a path. That is a security
     * property of the request and not a convenience: answering it makes the
     * daemon run git — including `merge-tree --write-tree`, which writes
     * objects — in the named directory, and every confined session has this
     * socket bound in. Keyed on a slug the daemon resolves by *searching* the
     * remembered set, the reachable directories are exactly the ones the user
     * already chose to remember. Sending a path here does not widen that; it
     * just gets a `bad_request`.
     */
    fun worktrees(project: String? = null): String =
        if (project.isNullOrEmpty()) {
            """{"cmd":"worktrees"}"""
        } else {
            """{"cmd":"worktrees","project":"${escape(project)}"}"""
        }

    /**
     * Start a session.
     *
     * `cwd`, `cols` and `rows` are the only required fields of `RunRequest`;
     * everything else has a serde default, and sending a key with a null in it
     * is not the same as leaving it out. So optional fields are appended only
     * when they have a value.
     */
    fun run(
        cwd: String,
        cols: Int,
        rows: Int,
        agent: String? = null,
        prompt: String? = null,
        worktree: String? = null,
    ): String = buildString {
        append("""{"cmd":"run","cwd":"""").append(escape(cwd)).append('"')
        append(""","cols":""").append(cols)
        append(""","rows":""").append(rows)
        if (!agent.isNullOrEmpty()) append(""","agent":"""").append(escape(agent)).append('"')
        if (!prompt.isNullOrEmpty()) append(""","prompt":"""").append(escape(prompt)).append('"')
        if (!worktree.isNullOrEmpty()) append(""","worktree":"""").append(escape(worktree)).append('"')
        append('}')
    }

    /**
     * JSON string escaping, including the one that is not about JSON.
     *
     * A newline inside a string would be `\n` in the output and therefore not
     * a real newline — but a raw control character in a path or a prompt would
     * be, and the wire refuses a control payload containing one. So control
     * characters are escaped as `\uXXXX` rather than passed through, and the
     * frame is always encodable.
     */
    fun escape(value: String): String = buildString(value.length + 8) {
        for (ch in value) {
            when {
                ch == '"' -> append("\\\"")
                ch == '\\' -> append("\\\\")
                ch == '\n' -> append("\\n")
                ch == '\r' -> append("\\r")
                ch == '\t' -> append("\\t")
                ch.code < 0x20 || ch.code == 0x7F -> append("\\u%04x".format(ch.code))
                else -> append(ch)
            }
        }
    }

    // ---- replies --------------------------------------------------------

    private fun tag(reply: String): String =
        runCatching { json.parseToJsonElement(reply) }.getOrNull()
            ?.let { (it as? JsonObject)?.get("reply")?.jsonPrimitive?.content } ?: ""

    /** Throws [AgentError] when the reply is one, and returns the object otherwise. */
    private fun require(reply: String, expected: String): JsonObject {
        val element = runCatching { json.parseToJsonElement(reply) }.getOrNull() as? JsonObject
            ?: throw AgentError("internal", "the machine sent something that is not a reply: ${reply.take(200)}")
        when (val t = element["reply"]?.jsonPrimitive?.content) {
            expected -> return element
            "error" -> throw AgentError(
                element["kind"]?.jsonPrimitive?.content ?: "internal",
                element["message"]?.jsonPrimitive?.content ?: "the runtime refused and did not say why",
            )
            else -> throw AgentError("internal", "expected a `$expected` reply and got `$t`")
        }
    }

    fun readSessions(reply: String): List<AgentSession> {
        val obj = require(reply, "sessions")
        val array = obj["sessions"] ?: return emptyList()
        return json.decodeFromJsonElement(kotlinx.serialization.builtins.ListSerializer(AgentSession.serializer()), array)
    }

    /**
     * One session, from an `info` or a `run`.
     *
     * The fields are read from the top level, **not** from a `"session"` key:
     * `Response::Session` is a newtype variant under an internal tag.
     */
    fun readSession(reply: String): AgentSession {
        val obj = require(reply, "session")
        return json.decodeFromJsonElement(AgentSession.serializer(), obj)
    }

    /**
     * The worktree rows, from a `worktrees` reply.
     *
     * Throws [AgentError] like every other parser here, with one case given a
     * name: see [isTooOld]. The rows are returned flat, in the daemon's own
     * order, because that order is what [Project.group] reads.
     */
    fun readWorktrees(reply: String): List<WorktreeStatus> {
        val obj = require(reply, "worktrees")
        val array = obj["worktrees"] ?: return emptyList()
        return json.decodeFromJsonElement(
            kotlinx.serialization.builtins.ListSerializer(WorktreeStatus.serializer()),
            array,
        )
    }

    /**
     * Whether this refusal is "that machine's runtime predates this verb".
     *
     * Refusal, absence and could-not-run are three different answers, and on
     * this socket two of them arrive as the same `kind`. `apex-agentd` answers
     * an unknown `cmd` from `serde_json::from_str::<Request>` failing, which
     * it reports as `bad_request` with the message `unparseable request:
     * unknown variant \`worktrees\`, expected one of …` (`main.rs:612`) — and
     * `worktrees` with a slug nothing matches *also* answers `bad_request`,
     * with `no remembered project with slug …`. A screen that showed the first
     * as "no projects" would be reporting a version skew as an empty machine.
     *
     * The `unknown variant` text is serde's, not APEX's, so this is keyed on
     * both halves: the daemon's own prefix and serde's phrase. It is only ever
     * used to choose *wording*; nothing is retried or skipped on the strength
     * of it.
     *
     * This matters today and not hypothetically: the image on this developer's
     * own machine is from 2026-09-05 and the verb landed on 2026-09-08, so
     * every `worktrees` request against it takes exactly this path.
     */
    fun isTooOld(error: AgentError): Boolean =
        error.kind == "bad_request" &&
            error.message.startsWith("unparseable request:") &&
            error.message.contains("unknown variant")

    fun readHello(reply: String): Hello {
        val obj = require(reply, "hello")
        return json.decodeFromJsonElement(Hello.serializer(), obj)
    }

    /**
     * An `ok`, or the error it actually was.
     *
     * The parsers are named `read*` and the builders are named for their
     * verbs, because `worktrees(String)` was both at once for an hour and
     * Kotlin silently resolved a list of requests to `List<Any>`.
     */
    fun readOk(reply: String) {
        if (tag(reply) == "ok") return
        require(reply, "ok")
    }

    /**
     * The daemon's scrollback window, and what a phone asks for.
     *
     * 256 KiB is the daemon's own `SCROLLBACK_BYTES` and its default, so this
     * is "everything there is" rather than a number chosen here. A phone on a
     * slow link pays for it once per attach; asking for less would mean
     * reconnecting into a terminal missing the part that mattered.
     */
    const val DEFAULT_REPLAY: Int = 256 * 1024
}
