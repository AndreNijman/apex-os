package com.apexos.remote.update

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp

/**
 * The one row in this app that talks about the app itself.
 *
 * It is on the machines screen, which is the home screen, because an update
 * the user never sees is an update that never happens — and because the
 * alternative, a settings page somebody has to think to open, is how a phone
 * ends up two protocol revisions behind the machine it is trying to reach.
 *
 * It is a ROW and not a dialog. Nothing here is urgent enough to interrupt
 * somebody who opened the app to look at an agent, and an app that blocks its
 * own front door to talk about its version is the kind of thing people learn
 * to dismiss without reading.
 *
 * [UpdateUi.Idle] draws nothing at all, which is also what every network
 * failure resolves to: no signal, a captive portal, a rate limit and a GitHub
 * outage are all silence, not a warning about something the user cannot fix.
 */
@Composable
fun UpdateBanner(
    state: UpdateUi,
    mayInstall: Boolean,
    onInstall: () -> Unit,
    onAllowSources: () -> Unit,
    onDismiss: () -> Unit,
) {
    when (state) {
        is UpdateUi.Idle -> Unit

        is UpdateUi.Available -> Banner(
            text = "APEX Remote ${state.offer.versionName} is available.",
            // Said here rather than left for the user to discover at the
            // prompt. Android will ask before it replaces anything, and a
            // person who has never sideloaded an app does not know that — so
            // the sentence that stops this feeling like a hijack is the one
            // promising they get the last word.
            detail = if (mayInstall) {
                "Android will ask you to confirm before it installs anything."
            } else {
                "Android needs your permission to let this app install updates. " +
                    "You can turn it off again afterwards."
            },
            alarming = false,
        ) {
            if (mayInstall) {
                TextButton(onClick = onInstall) { Text("Update") }
            } else {
                TextButton(onClick = onAllowSources) { Text("Allow") }
            }
            TextButton(onClick = onDismiss) {
                Text("Not now", color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }

        is UpdateUi.Working -> Banner(text = state.text, detail = null, alarming = false)

        is UpdateUi.Problem -> Banner(
            text = state.text,
            // The reassurance belongs next to the complaint. A checksum
            // failure sounds like the app has been tampered with, and the
            // thing the user most needs to know is that nothing happened to
            // the copy they already have.
            detail = "Your installed app has not changed.",
            alarming = true,
        ) {
            TextButton(onClick = onDismiss) { Text("Dismiss") }
        }
    }
}

@Composable
private fun Banner(
    text: String,
    detail: String?,
    alarming: Boolean,
    actions: @Composable () -> Unit = {},
) {
    val background =
        if (alarming) MaterialTheme.colorScheme.errorContainer else MaterialTheme.colorScheme.surfaceVariant
    val foreground =
        if (alarming) MaterialTheme.colorScheme.onErrorContainer else MaterialTheme.colorScheme.onSurfaceVariant
    Column(
        Modifier
            .fillMaxWidth()
            .background(background)
            .padding(horizontal = 20.dp, vertical = 12.dp)
            // A live region, so a screen reader announces this when it appears
            // instead of only when somebody happens to swipe onto it. The same
            // rule P1-060's accessibility work applied to the rest of the app:
            // a message that only sighted users receive is not a message.
            .semantics { liveRegion = LiveRegionMode.Polite },
    ) {
        Text(text, style = MaterialTheme.typography.bodyMedium, color = foreground)
        detail?.let {
            Text(it, style = MaterialTheme.typography.labelSmall, color = foreground)
        }
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) { actions() }
    }
}
