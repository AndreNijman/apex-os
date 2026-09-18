package com.apexos.remote.core.agent

import com.apexos.remote.core.link.FakeMachine
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertThrows
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.Timeout
import java.util.concurrent.atomic.AtomicInteger

/**
 * Reading the computer's clipboard, from the reply on the wire to the string.
 *
 * ## The half that had no test at all
 *
 * The REQUEST direction has been asserted from both ends since `clipboard`
 * landed: `AgentdRequestWireTest` builds it, and
 * `apexd/apex-agent-core/tests/android_requests_wire.rs` deserializes the same
 * fixture into `Request::Clipboard`. The REPLY direction had neither half.
 * [Agentd.readClipboard] was written, documented at length and shipped
 * untested, which meant the one thing nobody had checked was whether the field
 * it reads is the field the daemon writes.
 *
 * That gap had teeth. `Response::Clipboard` is internally tagged, so `text`
 * sits beside `"reply"` at the top level rather than in a nested object — the
 * same shape [Agentd.readSession] has a doc comment about. A parser that
 * reached one level too deep would answer the empty string for every reply,
 * and the empty string is a *legitimate* answer meaning the clipboard is
 * empty. The failure would have been silent, permanent, and shown to the user
 * as "the machine's clipboard is empty".
 *
 * `clipboard-replies.json` is the shared fixture and
 * `android_clipboard_wire.rs` is the other half: it asserts the same entries
 * are replies this crate could have sent, and that they round-trip. A Kotlin
 * test comparing a parser against a string a Kotlin author wrote would prove
 * only that the two agree with each other.
 */
class ClipboardReplyTest {

    private val json = Json { ignoreUnknownKeys = true }

    private val fixture: JsonObject by lazy {
        val stream = javaClass.classLoader.getResourceAsStream("clipboard-replies.json")
            ?: error(
                "clipboard-replies.json is missing from the test resources. This test asserts " +
                    "an AGREEMENT with the daemon's own fixture, so a run that could not read " +
                    "it must fail rather than report a parser that checked nothing.",
            )
        json.parseToJsonElement(stream.bufferedReader().readText()).jsonObject
    }

    /** The named cases, with the `_comment` prose dropped. */
    private fun cases(): List<Pair<String, String>> =
        fixture.entries
            .filterNot { it.key.startsWith("_") }
            .map { it.key to it.value.toString() }

    /** What the fixture says the text of a named case is, read independently. */
    private fun expected(name: String): String =
        fixture[name]!!.jsonObject["text"]!!.jsonPrimitive.content

    @Test
    fun `the fixture was actually read and still has the cases the parser's behaviour rests on`() {
        // The guard against every assertion below passing because the resource
        // stopped loading or the filter stopped matching. A loop over nothing
        // asserts nothing.
        val names = cases().map { it.first }
        assertTrue(names.size >= 5, "the clipboard reply fixture collapsed to $names")
        for (required in listOf("text", "empty", "multiline")) {
            assertTrue(
                required in names,
                "the fixture lost its `$required` case, which is one of the three this " +
                    "parser's documented behaviour depends on; have: $names",
            )
        }
    }

    @Test
    fun `every reply in the shared fixture is read back verbatim`() {
        for ((name, reply) in cases()) {
            assertEquals(
                expected(name),
                Agentd.readClipboard(reply),
                "`$name` did not survive the parser",
            )
        }
    }

    @Test
    fun `an empty clipboard is a real answer and not an error`() {
        // The distinction the parser's doc comment promises and the reason the
        // view model routes this to `notice` and never to `failure`. A machine
        // with nothing copied on it has not refused anything.
        assertEquals("", Agentd.readClipboard(fixture["empty"].toString()))
    }

    @Test
    fun `text is read from the top level, not from a nested object`() {
        // The shape defect this file exists for. A nested reply is NOT what
        // the daemon sends, so the parser must not find anything in it —
        // asserted explicitly, because a parser that happened to read both
        // would hide the day the daemon changed.
        val nested = """{"reply":"clipboard","clipboard":{"text":"nested"}}"""
        assertEquals(
            "",
            Agentd.readClipboard(nested),
            "the parser found `text` inside a nested object. Response::Clipboard is " +
                "internally tagged and puts it at the top level; a parser that reads both " +
                "shapes cannot tell a daemon change from an empty clipboard.",
        )
    }

    @Test
    fun `whitespace is content and is not trimmed away`() {
        // Somebody who copied indentation copied indentation. Trimming here
        // would corrupt a pasted code block, and the daemon already declines
        // to add a trailing newline for the same reason.
        assertEquals("   ", Agentd.readClipboard(fixture["spaces_only"].toString()))
    }

    @Test
    fun `a refusal is thrown rather than returned as empty text`() {
        // The other half of the empty-versus-refused contract. If a refusal
        // came back as "" the user would be told the machine's clipboard was
        // empty when the machine had actually declined, and no amount of
        // retrying would ever explain why.
        val refusal = """{"reply":"error","kind":"permission_denied","message":"a session may not read this machine's clipboard"}"""
        val error = assertThrows(AgentError::class.java) { Agentd.readClipboard(refusal) }
        assertEquals("permission_denied", error.kind)
        assertTrue(
            error.message.contains("may not read this machine's clipboard"),
            "the daemon's own sentence has to reach the user: ${error.message}",
        )
        assertFalse(
            Agentd.isTooOld(error),
            "a refusal must not be reported as a version skew, which would tell the user " +
                "to run `sudo apex update` for a permission problem",
        )
    }

    @Test
    fun `a runtime older than the verb is recognised as a version skew`() {
        // The case the view model turns into "run `sudo apex update`" rather
        // than into a permission message. `Request::Clipboard` was added
        // without a PROTOCOL_VERSION bump precisely because this failure loses
        // a feature and never a restriction — so it has to be *recognisable*,
        // or that decision costs the user a confusing error instead.
        val old = """{"reply":"error","kind":"bad_request","message":"unparseable request: unknown variant `clipboard`, expected one of `hello`, `list`"}"""
        val error = assertThrows(AgentError::class.java) { Agentd.readClipboard(old) }
        assertTrue(Agentd.isTooOld(error), "an old runtime's refusal was not recognised as one")
    }

    @Test
    fun `a reply for a different verb is refused rather than mined for a text field`() {
        val wrong = """{"reply":"logs","id":3,"text":"output"}"""
        val error = assertThrows(AgentError::class.java) { Agentd.readClipboard(wrong) }
        assertTrue(
            error.message.contains("clipboard"),
            "the error should name the reply that was expected: ${error.message}",
        )
    }

    // ---- the whole client path, over a real multiplexer ------------------

    @Test
    @Timeout(30)
    fun `MachineLink asks for the clipboard with exactly the request the daemon parses`() {
        // End to end for the client half: the builder, the frame, the wire
        // format (FakeMachine encodes and decodes every frame), the reply and
        // the parser. The assertion on what the machine RECEIVED is the point
        // — a link that returned the right string while sending the wrong
        // request would pass every unit test in this file.
        val reply = fixture["multiline"].toString()
        val machine = FakeMachine(name = "l16", control = { reply })
        MachineLink(connect = { machine.open() }).use { link ->
            assertEquals(expected("multiline"), link.clipboard())
        }

        assertEquals(
            listOf("""{"cmd":"clipboard"}"""),
            machine.requests.toList(),
            "the machine was asked something other than `clipboard`, or asked twice",
        )
    }

    @Test
    @Timeout(30)
    fun `a clipboard read is retried on a lost connection, unlike a typed reply`() {
        // The claim `MachineLink.clipboard`'s doc comment makes, exercised
        // rather than asserted in prose. It is a QUESTION: asking twice reads
        // the clipboard twice and changes nothing at the machine, so a caller
        // who lost the reply is better served by a second ask than an error.
        // `input` is deliberately the opposite — replaying it would type a
        // user's sentence into an agent twice — and `revoke` and `signal`
        // likewise.
        val reply = fixture["text"].toString()
        val asks = AtomicInteger(0)
        lateinit var machine: FakeMachine
        machine = FakeMachine(
            name = "l16",
            control = {
                if (asks.incrementAndGet() == 1) {
                    // The plug, with the request already delivered: the shape
                    // where the daemon may or may not have acted and only the
                    // caller knows whether repeating is safe. For a question
                    // it always is.
                    machine.hangUp()
                    """{"reply":"ok"}"""
                } else {
                    reply
                }
            },
        )

        MachineLink(connect = { machine.open() }).use { link ->
            assertEquals(expected("text"), link.clipboard())
            assertEquals(2, link.connections, "the retry did not open a fresh connection")
        }
        assertEquals(2, asks.get(), "the machine was asked once, so nothing was retried")
    }
}
