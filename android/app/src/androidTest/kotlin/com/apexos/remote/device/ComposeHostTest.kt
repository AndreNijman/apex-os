package com.apexos.remote.device

import androidx.compose.material3.Text
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/**
 * The floor under every other Compose assertion on this phone: that a
 * composition made by the test rule really reaches the screen and really
 * appears in the semantics tree.
 *
 * It exists because the first version of the accessibility suite PASSED while
 * checking nothing — `tryPerformAccessibilityChecks()` on an empty node
 * collection is a no-op, so a test that composed nothing reported success. A
 * check that cannot fail when the thing it checks is absent is the dominant
 * defect family in this repository's gates, and this file is the guard against
 * it here: if this test fails, every other Compose result in this module is
 * meaningless and says so.
 */
@RunWith(AndroidJUnit4::class)
class ComposeHostTest {
    @get:Rule
    val compose = createAndroidComposeRule<ComposeHostActivity>()

    @Test
    fun a_composition_made_by_the_test_rule_reaches_the_semantics_tree() {
        compose.setContent { Text("a-node-that-must-exist") }
        compose.waitForIdle()
        compose.onNodeWithText("a-node-that-must-exist").assertIsDisplayed()
    }
}
