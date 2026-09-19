package com.apexos.remote.device

import android.content.pm.ActivityInfo
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.test.isRoot
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onRoot
import androidx.compose.ui.unit.Density
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.filters.LargeTest
import com.apexos.remote.ui.theme.ApexRemoteTheme
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/**
 * P1-060 criterion 2's layout half: large text and rotation, on the phone.
 *
 * ## Large text is provided, not set on the device
 *
 * `settings put system font_scale 2.0` would change the phone for every app
 * and leave a value behind if the run died; providing `LocalDensity` with a
 * `fontScale` of 2 changes exactly this composition and nothing else, and two
 * runs of this suite cannot disagree because of what the last one left set.
 * It is the same number Android's largest accessibility setting produces.
 *
 * This is the on-device confirmation of `labelColumnWidth()`: three
 * `Modifier.width(110.dp)` label columns clipped at any font scale above 1.0,
 * and until now the only evidence for the fix was a source scan.
 */
@RunWith(AndroidJUnit4::class)
@LargeTest
class LayoutOnDeviceTest {

    @get:Rule
    val compose = createAndroidComposeRule<ComposeHostActivity>()

    /** The largest scale Android's accessibility settings offer. */
    private val hugeText = 2.0f

    private fun walkScreens(fontScale: Float? = null, check: (String) -> Unit) {
        var index by mutableIntStateOf(0)
        compose.setContent {
            val density = LocalDensity.current
            val scaled = if (fontScale == null) {
                density
            } else {
                Density(density.density, fontScale)
            }
            CompositionLocalProvider(LocalDensity provides scaled) {
                ApexRemoteTheme { Screens.all[index].second() }
            }
        }
        for (i in Screens.all.indices) {
            index = i
            compose.waitForIdle()
            val name = Screens.all[i].first
            val nodes = compose.onAllNodes(isRoot()).fetchSemanticsNodes()
            assertTrue("$name composed no semantics at all", nodes.isNotEmpty())
            check(name)
        }
    }

    /**
     * Nothing a person can read may be laid out with no width or no height.
     *
     * The failure this catches is the one `labelColumnWidth()` fixed: a fixed
     * `dp` column holding text that grew, so the text is measured, placed, and
     * then has nothing left to occupy. A zero dimension is how that arrives in
     * the semantics tree.
     */
    private fun assertNothingCollapsed(name: String, scale: String) {
        val root = compose.onRoot().fetchSemanticsNode()
        val collapsed = mutableListOf<String>()
        fun walk(node: androidx.compose.ui.semantics.SemanticsNode) {
            val text = node.config.getOrNull(SemanticsProperties.Text)
                ?.joinToString(" ") { it.text }
                ?.trim()
                .orEmpty()
            if (text.isNotEmpty() && (node.size.width == 0 || node.size.height == 0)) {
                collapsed += "${text.take(40)} (${node.size.width}x${node.size.height})"
            }
            node.children.forEach(::walk)
        }
        walk(root)
        assertTrue(
            "$name at $scale lays out ${collapsed.size} piece(s) of text with no room: $collapsed",
            collapsed.isEmpty(),
        )
    }

    @Test
    fun every_screen_survives_the_largest_text_android_offers() {
        walkScreens(fontScale = hugeText) { name -> assertNothingCollapsed(name, "font scale 2.0") }
    }

    @Test
    fun every_screen_lays_out_at_the_default_scale_too() {
        // The control arm. Without it, a run in which NOTHING lays out — a
        // host that never reached the screen, say — would pass the test above
        // by having no text to collapse, and the large-text claim would rest
        // on nothing.
        walkScreens { name -> assertNothingCollapsed(name, "the default font scale") }
    }

    @Test
    fun every_screen_lays_out_in_landscape() {
        // The activity declares the same `configChanges` as `MainActivity`, so
        // it handles the rotation itself rather than being recreated — which
        // is what the app does and therefore what is worth asserting. What is
        // NOT claimed here is that a recreated activity restores its state:
        // this app's one activity is never recreated by a rotation, and a test
        // of a path the app does not take would be a test of nothing.
        compose.activity.requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_LANDSCAPE
        compose.waitForIdle()
        var sawLandscape = false
        walkScreens { name ->
            val configuration = compose.activity.resources.configuration
            sawLandscape = sawLandscape || configuration.screenWidthDp > configuration.screenHeightDp
            assertNothingCollapsed(name, "landscape")
        }
        assertTrue(
            "the phone never actually rotated, so this asserted nothing about landscape",
            sawLandscape,
        )
        compose.activity.requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED
    }
}
