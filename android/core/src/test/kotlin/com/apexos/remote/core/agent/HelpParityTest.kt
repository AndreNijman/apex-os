package com.apexos.remote.core.agent

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

/**
 * "In-app help equivalent to desktop Agents/Workspaces help", made checkable.
 *
 * P1-060's last criterion is a comparison, and a comparison nobody runs is an
 * opinion. This parses the desktop's own `AgentHelpContent.qml` — vendored
 * beside `agentstate.js` and `Colors.qml`, and diffed against a live checkout
 * by `tools/check-agent-state.sh` — and asserts that every section the desktop
 * teaches has a counterpart here.
 *
 * ## Equivalent, not identical, and the difference must be written down
 *
 * The two clients do not have the same powers, so equal *words* would be
 * wrong: repeating the desktop's instructions would tell somebody holding a
 * phone to do things they cannot do from it. What is asserted is therefore
 * coverage of the desktop's SECTIONS, plus the three places where this app is
 * less able than the desktop and has to say so rather than stay quiet. A
 * section that genuinely does not apply has to be listed in [DESKTOP_ONLY]
 * with a reason, so dropping one is a decision somebody wrote down instead of
 * a gap nobody noticed.
 */
class HelpParityTest {

    /**
     * Desktop sections with no counterpart here, each with the reason.
     *
     * Empty today. It is not decoration: it is the mechanism that makes
     * removing a section cost somebody a sentence.
     */
    private val DESKTOP_ONLY: Map<String, String> = emptyMap()

    private val desktop: String by lazy {
        HelpParityTest::class.java.getResourceAsStream("/desktop/AgentHelpContent.qml")
            ?.bufferedReader()?.readText()
            ?: error(
                "the vendored copy of apex-shell's AgentHelpContent.qml is missing. This test " +
                    "asserts a COMPARISON, so a run that could not read the other side must " +
                    "fail rather than report agreement it never checked.",
            )
    }

    /** `id: "start",` → `start`, in the order the desktop declares them. */
    private val desktopSections: List<String> by lazy {
        Regex("""^\s*id: "([a-z]+)",""", RegexOption.MULTILINE)
            .findAll(desktop)
            .map { it.groupValues[1] }
            .toList()
    }

    @Test
    fun `the desktop guide was actually parsed`() {
        // The guard against the whole suite passing because a regex stopped
        // matching. Seven sections is what apex-shell ships; a change there is
        // supposed to break this and be looked at, not be absorbed silently.
        assertEquals(
            listOf("start", "projects", "review", "sandbox", "remote", "keys", "stuck"),
            desktopSections,
            "the desktop guide's sections changed, or the parse stopped working. Either way " +
                "this app's guide has to be looked at rather than assumed still equivalent.",
        )
    }

    @Test
    fun `every desktop section has a counterpart here`() {
        for (id in desktopSections) {
            if (id in DESKTOP_ONLY) continue
            assertTrue(
                id in Help.covered,
                "the desktop teaches `$id` and this app's guide has nothing answering to it. " +
                    "Either write it, or list it in DESKTOP_ONLY with the reason it does not " +
                    "apply to a phone.",
            )
        }
    }

    @Test
    fun `this guide invents no section the desktop does not have`() {
        // Not tidiness. A section here with no desktop counterpart is a feature
        // being taught in one place, which is how the two ends stop being one
        // product.
        for (s in Help.sections) {
            assertTrue(
                s.desktopId in desktopSections,
                "`${s.desktopId}` is not a section of the desktop guide",
            )
        }
        assertEquals(
            Help.sections.size,
            Help.sections.map { it.desktopId }.toSet().size,
            "two sections answer to the same desktop section",
        )
    }

    @Test
    fun `the order follows the desktop, so the two read the same way`() {
        assertEquals(
            desktopSections.filterNot { it in DESKTOP_ONLY },
            Help.sections.map { it.desktopId },
            "the sections are in a different order from the desktop's",
        )
    }

    @Test
    fun `every section says something`() {
        for (s in Help.sections) {
            assertTrue(s.title.isNotBlank(), "${s.desktopId} has no title")
            assertTrue(s.blocks.isNotEmpty(), "${s.desktopId} is an empty section")
            for (b in s.blocks) {
                assertTrue(b.text.isNotBlank(), "${s.desktopId} has an empty block")
                if (b.kind == Help.Kind.KV) {
                    assertTrue(b.term.isNotBlank(), "${s.desktopId} has a kv with no term")
                }
            }
        }
    }

    /**
     * The three things this app cannot do that the desktop can, each named in
     * the guide rather than left for the user to discover by pressing.
     *
     * Keyed on the section it belongs in, so moving the sentence somewhere it
     * will not be read fails too. The substrings are deliberately short and
     * factual: asserting a whole sentence would make every edit a test failure,
     * and asserting nothing would let the sentence quietly disappear.
     */
    @Test
    fun `the guide names what this app cannot do, in the section where it matters`() {
        val required = mapOf(
            // A checkpoint can be asked for here and never restored here: there
            // is no verb on this connection that reads one or reverses one.
            "review" to listOf("cannot be restored from this phone", "apex agent undo"),
            // Every verb in the vocabulary is a root operation and APEX reserves
            // approving one for a person at the machine. A phone cannot even
            // deny.
            "sandbox" to listOf("cannot approve anything", "cannot even deny"),
            // The runtime is a service on the machine and this app cannot start
            // it.
            "start" to listOf("apex agent enable"),
            // The relay has never been deployed, so off the machine's network
            // this app reaches nothing.
            "remote" to listOf("no deployment yet"),
            // No verb carries file content, so there is no photo or file upload.
            "keys" to listOf("carry file content"),
            // A machine older than a verb answers the same error a refusal
            // does, and telling them apart is the point.
            "stuck" to listOf("too old"),
        )
        for ((id, phrases) in required) {
            val section = Help.sections.firstOrNull { it.desktopId == id }
                ?: error("no `$id` section to check")
            val text = section.blocks.joinToString(" ") { it.text }
            for (phrase in phrases) {
                assertTrue(
                    text.contains(phrase, ignoreCase = true),
                    "the `$id` section no longer says `$phrase`. That sentence is the only " +
                        "place a user learns this app cannot do something the desktop can, " +
                        "and without it they find out by pressing something that refuses.",
                )
            }
        }
    }

    @Test
    fun `nothing in the guide claims a capability this build does not have`() {
        // The inverse assertion, and the cheaper one to get wrong. A guide is
        // written once and read after the code has moved.
        val all = Help.sections.flatMap { it.blocks }.joinToString(" ") { it.text }
        val forbidden = listOf(
            "approve from your phone",
            "restore the checkpoint here",
            "upload a photo",
            "attach a file",
        )
        for (claim in forbidden) {
            assertFalse(
                all.contains(claim, ignoreCase = true),
                "the guide offers `$claim`, which nothing in this build can do",
            )
        }
    }

    @Test
    fun `commands in the guide are run on the computer and say so`() {
        // Every `cmd` block here is something to type at the machine, never on
        // the phone, and a reader who types one into a terminal app on their
        // phone has been misled by this guide.
        val commands = Help.sections.flatMap { it.blocks }
            .filter { it.kind == Help.Kind.CMD }
            .map { it.text }
        assertTrue(commands.isNotEmpty(), "the guide gives no commands at all")
        for (c in commands) {
            assertTrue(
                c.startsWith("apex ") || c.startsWith("sudo apex "),
                "`$c` is not an apex command, so a reader cannot tell it belongs at the " +
                    "computer",
            )
        }
    }
}
