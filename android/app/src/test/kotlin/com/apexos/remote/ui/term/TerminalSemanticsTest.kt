package com.apexos.remote.ui.term

import androidx.compose.ui.semantics.AccessibilityAction
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.SemanticsPropertyKey
import androidx.compose.ui.semantics.SemanticsPropertyReceiver
import androidx.compose.ui.text.AnnotatedString
import com.apexos.remote.core.term.Terminal
import com.apexos.remote.core.term.TerminalReading
import com.apexos.remote.core.term.Viewport
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertNotNull
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

/**
 * What the terminal actually puts in the semantics tree — read back, not
 * grepped for.
 *
 * ## Why a fake receiver, and what it is worth
 *
 * `AccessibilityStaticsTest` decides the part of this that a source scan can,
 * and says in as many words that a scan is not the criterion. This is the next
 * rung up and it needs no phone: `Modifier.semantics { }` takes a lambda on
 * `SemanticsPropertyReceiver`, that interface has one method, and a test can
 * implement it and read back every property the production code set. So these
 * assertions are about the values Android will be handed, not about whether a
 * file mentions the word `contentDescription`.
 *
 * What it still cannot decide is whether TalkBack says something useful with
 * them. That is `AccessibilityOnDeviceTest`'s, and it needs a phone.
 */
class TerminalSemanticsTest {

    /** A `SemanticsPropertyReceiver` that remembers rather than composes. */
    private class Captured : SemanticsPropertyReceiver {
        val values = mutableMapOf<SemanticsPropertyKey<*>, Any?>()

        override fun <T> set(key: SemanticsPropertyKey<T>, value: T) {
            values[key] = value
        }

        @Suppress("UNCHECKED_CAST")
        fun <T> get(key: SemanticsPropertyKey<T>): T? = values[key] as T?

        val text: String?
            get() = get(SemanticsProperties.Text)?.joinToString("\n") { it.text }

        val onClick: AccessibilityAction<() -> Boolean>?
            get() = get(SemanticsActions.OnClick)
    }

    private val terminal = Terminal(cols = 20, rows = 6)
    private val viewport = Viewport(terminal, height = 6)

    private fun capture(
        onActivate: (() -> Boolean)? = null,
        actions: List<CustomAccessibilityAction> = emptyList(),
    ): Captured = Captured().apply {
        terminalSemantics(TerminalReading.of(viewport.snapshot()), onActivate, actions)
    }

    @Test
    fun `the grid reaches the semantics tree as text`() {
        // The whole of blocker two, in one assertion. `TerminalView` is a
        // `Canvas`: it paints with `drawText`, so before this there was
        // nothing on the screen for a reader to find — an empty subtree, and
        // TalkBack announcing nothing on a terminal full of output.
        terminal.feed("cargo test\r\n3407 passed\r\n")
        val node = capture()
        assertEquals("cargo test\n3407 passed", node.text)
    }

    @Test
    fun `it is text and not a content description, so it can be navigated`() {
        // TalkBack moves through a node's TEXT at the granularity the user
        // chose — line, word, character — and reading a terminal line by line
        // is the entire point. A `contentDescription` is one utterance that
        // can only be replayed from the beginning, which turns a forty-line
        // build log into one forty-line sentence.
        terminal.feed("one\r\ntwo\r\n")
        val node = capture()
        assertNotNull(node.get(SemanticsProperties.Text))
        assertNull(
            node.get(SemanticsProperties.ContentDescription),
            "the buffer must be text; a description cannot be read line by line",
        )
    }

    @Test
    fun `the node says where the view is, as a state and not as more text`() {
        // Mixed into the text it would be read out as another line of output
        // on every pass through the screen.
        terminal.feed("hello\r\n")
        val node = capture()
        val state = node.get(SemanticsProperties.StateDescription)
        assertNotNull(state)
        assertTrue(state!!.contains("Cursor on line 2"), state)
        assertFalse(node.text!!.contains("Cursor on line"), "position is not part of the buffer")
    }

    @Test
    fun `the terminal is activatable, and the label is what a reader says`() {
        // A tap on this screen raises the keyboard, and it used to be a bare
        // `pointerInput` — invisible to the semantics tree. A reader could not
        // activate it, and announced nothing when it tried.
        var typed = false
        val node = capture(onActivate = { typed = true; true })
        val click = node.onClick
        assertNotNull(click)
        assertEquals("type", click!!.label, "TalkBack reads this as \"double tap to <label>\"")
        assertTrue(click.action!!.invoke())
        assertTrue(typed, "the action has to do the thing, not merely exist")
    }

    @Test
    fun `a terminal that cannot take input is not announced as activatable`() {
        assertNull(capture(onActivate = null).onClick)
    }

    @Test
    fun `the two actions that are otherwise a target nobody can see are offered`() {
        val jump = CustomAccessibilityAction("Back to the live output") { true }
        val copy = CustomAccessibilityAction("Copy what is on screen") { true }
        val node = capture(actions = listOf(jump, copy))
        assertEquals(
            listOf("Back to the live output", "Copy what is on screen"),
            node.get(SemanticsActions.CustomActions)?.map { it.label },
        )
    }

    @Test
    fun `the grid is NOT a live region`() {
        // The assertion that keeps the feature usable. A live region is
        // re-read whenever its contents change, and this node's contents are
        // the whole screen: marking it one would read forty lines aloud for
        // every byte that arrived, each announcement discarding the last. The
        // announcement node below is the answer instead.
        terminal.feed("output\r\n")
        assertNull(capture().get(SemanticsProperties.LiveRegion))
    }

    @Test
    fun `an empty terminal still names itself`() {
        // Because it is clickable, and a clickable node with no words is
        // announced as "button" and nothing else.
        val node = capture(onActivate = { true })
        assertTrue(node.text!!.isNotBlank())
    }

    // ---- the node that interrupts ----------------------------------------

    @Test
    fun `an announcement is a polite live region carrying a description`() {
        val node = Captured().apply { terminalAnnouncement("build finished") }
        assertEquals(
            listOf("build finished"),
            node.get(SemanticsProperties.ContentDescription),
        )
        assertEquals(LiveRegionMode.Polite, node.get(SemanticsProperties.LiveRegion))
    }

    @Test
    fun `an announcement carries no text, so it is not a second copy of the screen`() {
        // A live region is announced by its DESCRIPTION changing. Text on this
        // node would put the utterance into the tree twice — once to be read
        // out, once to be swiped through — and a reader exploring the screen
        // would find the last announcement sitting beside the buffer it came
        // from.
        val node = Captured().apply { terminalAnnouncement("build finished") }
        assertNull(node.get(SemanticsProperties.Text))
    }

    @Test
    fun `the announcement is exactly what the announcer decided`() {
        // No re-wording between the policy and the node: the reason the
        // policy can be tested in `:core` is that nothing downstream of it
        // changes what it said.
        val decided = "17 earlier lines.\ncompiling main.rs"
        val node = Captured().apply { terminalAnnouncement(decided) }
        assertEquals(listOf(decided), node.get(SemanticsProperties.ContentDescription))
    }

    @Test
    fun `AnnotatedString is what the text key holds`() {
        // The key is `List<AnnotatedString>`, and a build that set a plain
        // String would fail at the cast rather than at the assertion — so this
        // reads the raw value rather than the convenience above.
        terminal.feed("plain")
        val raw = capture().values[SemanticsProperties.Text]
        assertTrue(raw is List<*> && raw.all { it is AnnotatedString }, "got $raw")
    }
}
