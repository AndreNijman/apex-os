package com.apexos.remote.device

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.test.SemanticsNodeInteractionsProvider
import androidx.compose.ui.test.isRoot
import androidx.compose.ui.test.junit4.accessibility.enableAccessibilityChecks
import androidx.compose.ui.test.junit4.createAndroidComposeRule

import androidx.compose.ui.test.onRoot
import androidx.compose.ui.test.tryPerformAccessibilityChecks
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.filters.LargeTest
import com.apexos.remote.ui.theme.ApexRemoteTheme
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/**
 * P1-060 criterion 2's accessibility half, on the phone.
 *
 * `AccessibilityStaticsTest` in `:app`'s JVM suite decides the narrow part a
 * source scan can — no icon-only control, no hardcoded `sp` — and says in as
 * many words that it is not the criterion. This is the rest of it: the
 * Accessibility Test Framework run against the **real semantics tree**, on a
 * real display, at a real density, which is the tree a screen reader walks.
 *
 * ## Why the checks are performed explicitly
 *
 * `enableAccessibilityChecks()` runs ATF as a side effect of `perform*`
 * actions, so a test that composes a screen and asserts nothing about it runs
 * NO checks and passes — a green that looked at nothing, which is the dominant
 * defect family in this repository's gates. Every screen below therefore calls
 * `tryPerformAccessibilityChecks()` by hand.
 */
@RunWith(AndroidJUnit4::class)
@LargeTest
class AccessibilityOnDeviceTest {

    @get:Rule
    val compose = createAndroidComposeRule<ComposeHostActivity>()

    /**
     * Walk every screen inside ONE `setContent`.
     *
     * `setContent` may be called once per test — a second call throws "has
     * already set content" — so the screen under test is a piece of Compose
     * state the host recomposes on, rather than a fresh content tree each time.
     * That is also closer to what the app does: `NavHost` swaps a destination
     * inside one activity.
     */
    private fun onEachScreen(check: (name: String) -> Unit) {
        var index by mutableIntStateOf(0)
        compose.setContent { ApexRemoteTheme { Screens.all[index].second() } }
        for (i in Screens.all.indices) {
            index = i
            compose.waitForIdle()
            val name = Screens.all[i].first
            // BEFORE the caller's check, every time. The first version of this
            // suite reported a pass while composing nothing:
            // `tryPerformAccessibilityChecks()` on an empty node collection is
            // a no-op, so "no compose hierarchy at all" was indistinguishable
            // from "every screen is fine". The phone's screen was asleep behind
            // a secure keyguard and the host activity was being destroyed a
            // frame after launch. A check that cannot fail when its subject is
            // absent is worth nothing, so the subject's presence is asserted.
            val nodes = compose.onAllNodes(isRoot()).fetchSemanticsNodes()
            if (nodes.isEmpty()) {
                throw AssertionError(
                    "$name composed no semantics at all, so nothing below this line " +
                        "looked at anything. Check that the phone is awake and that " +
                        "ComposeHostActivity still declares showWhenLocked.",
                )
            }
            check(name)
        }
    }

    @Test
    fun every_screen_passes_the_accessibility_test_framework() {
        compose.enableAccessibilityChecks()
        onEachScreen { name ->
            try {
                // `onAllNodes(isRoot())` rather than `onRoot()`: the checker
                // is defined on a COLLECTION, and it is the collection form
                // that walks every node under each match.
                compose.onAllNodes(isRoot()).tryPerformAccessibilityChecks()
            } catch (e: Throwable) {
                throw AssertionError("$name fails an accessibility check: ${e.message}", e)
            }
        }
    }

    @Test
    fun every_control_a_screen_reader_could_activate_has_something_to_say() {
        // The TalkBack question, asked of the tree TalkBack actually reads: a
        // node carrying `OnClick` and carrying neither text nor a content
        // description is announced as "button" and nothing else.
        compose.enableAccessibilityChecks()
        onEachScreen { name ->
            val silent = clickableNodesWithoutLabels(compose)
            if (silent.isNotEmpty()) {
                throw AssertionError(
                    "$name has ${silent.size} activatable control(s) a screen reader cannot " +
                        "name: $silent",
                )
            }
        }
    }

    /** Whether anything under this node carries words a screen reader reads. */
    private fun androidx.compose.ui.semantics.SemanticsNode.speaksAnywhereBelow(): Boolean =
        children.any { child ->
            child.config.getOrNull(SemanticsProperties.Text)?.any { it.text.isNotBlank() } == true ||
                child.config.getOrNull(SemanticsProperties.ContentDescription)
                    ?.any { it.isNotBlank() } == true ||
                child.speaksAnywhereBelow()
        }

    private fun clickableNodesWithoutLabels(
        provider: SemanticsNodeInteractionsProvider,
    ): List<String> {
        val root = provider.onRoot().fetchSemanticsNode()
        val silent = mutableListOf<String>()
        fun walk(node: androidx.compose.ui.semantics.SemanticsNode) {
            val config = node.config
            val clickable = config.contains(SemanticsActions.OnClick)
            if (clickable) {
                val described = config.getOrNull(SemanticsProperties.ContentDescription)
                    ?.any { it.isNotBlank() } == true
                val texted = config.getOrNull(SemanticsProperties.Text)
                    ?.any { it.text.isNotBlank() } == true
                // A parent whose own label is empty is fine when a DESCENDANT
                // supplies one: that is how Compose composes a labelled button
                // out of a clickable Row and a Text, and the merged node a
                // screen reader reads carries those words. The whole subtree
                // and not just the immediate children — a label two levels down
                // still reaches the reader, and stopping at one level would
                // report controls that are in fact named.
                val descendantSpeaks = node.speaksAnywhereBelow()
                if (!described && !texted && !descendantSpeaks) {
                    silent += "node ${node.id} at ${node.boundsInRoot}"
                }
            }
            node.children.forEach(::walk)
        }
        walk(root)
        return silent
    }
}
