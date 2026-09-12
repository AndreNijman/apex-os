package com.apexos.remote.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.fragment.app.FragmentActivity
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.rememberNavController
import com.apexos.remote.ui.theme.ApexRemoteTheme

/**
 * The whole app, above the screens.
 *
 * Navigation is two destinations and a gate, which is the entire shape of
 * P1-053: a list of machines, the screen that adds one, and a lock in front of
 * both. P1-054 and P1-055 add destinations under the machine row rather than
 * changing this.
 */
object Destinations {
    const val MACHINES = "machines"
    const val PAIRING = "pairing"
}

@Composable
fun ApexRemoteApp(
    activity: FragmentActivity,
    deviceName: String,
    /** A payload the app was launched with, from an `apex-remote:` link. */
    launchPayload: String? = null,
    /** Called once the payload has been handed to a screen, so it fires once. */
    onPayloadConsumed: () -> Unit = {},
    viewModel: RemoteViewModel = viewModel(),
) {
    val state by viewModel.state.collectAsStateWithLifecycle()

    ApexRemoteTheme(useDynamicColour = state.settings.dynamicColour) {
        val navigation = rememberNavController()

        if (!state.unlocked) {
            LockScreen(
                failure = state.failure,
                onUnlock = { viewModel.unlockApp(activity) },
            )
            // Asked once the state is known, rather than behind a button nobody
            // would choose to press. Keyed on `loading` because the view model
            // refuses to prompt before it has decided whether this phone can
            // authenticate at all — so the effect has to run again once it has.
            LaunchedEffect(state.loading) { viewModel.unlockApp(activity) }
            // And nothing below this line runs while locked, which is why the
            // deep-link effect is inside the graph rather than out here:
            // `NavController.navigate` before a `NavHost` has composed throws
            // "Navigation graph has not been set", and a cold launch from an
            // `apex-remote:` link is exactly that moment.
            return@ApexRemoteTheme
        }

        NavHost(navigation, startDestination = Destinations.MACHINES) {
            composable(Destinations.MACHINES) {
                MachinesScreen(
                    state = state,
                    onPair = { navigation.navigate(Destinations.PAIRING) },
                    onConnect = { viewModel.connect(activity, it) },
                    onForget = { viewModel.forget(it) },
                    onDynamicColour = { viewModel.setDynamicColour(it) },
                    onLock = { viewModel.lock() },
                    onDismiss = { viewModel.dismiss() },
                )
            }
            composable(Destinations.PAIRING) {
                PairingScreen(
                    state = state,
                    // Handed to the screen, not acted on. It arrives in the
                    // paste box with the explanation beside it, and pairing
                    // still takes a deliberate press.
                    initialPayload = launchPayload,
                    onPayload = { payload ->
                        viewModel.pair(activity, payload, deviceName)
                    },
                    onBack = { navigation.popBackStack() },
                    onDismiss = { viewModel.dismiss() },
                )
            }
        }

        // A link that carries a pairing code opens the screen that can explain
        // what accepting it would mean. It is deliberately NOT paired on
        // arrival: a URL that paired a phone by being opened would be a URL
        // worth sending somebody. Inside the graph, because navigating before
        // the `NavHost` has composed throws.
        LaunchedEffect(launchPayload) {
            if (launchPayload != null) {
                navigation.navigate(Destinations.PAIRING)
                // Consumed, so that locking and unlocking again does not take
                // the user back to a code that has since expired.
                onPayloadConsumed()
            }
        }

        // A pairing that worked takes the user back to the list it was added
        // to, which is where the next thing they want to do is.
        LaunchedEffect(state.machines.size) {
            if (state.message != null && navigation.currentDestination?.route == Destinations.PAIRING) {
                navigation.popBackStack()
            }
        }
    }
}

/**
 * The front door.
 *
 * Nothing behind it is a secret — the store holds public keys and machine
 * names — and that is worth saying out loud on the screen rather than implying
 * a strength this does not have. What it stops is a phone picked up off a table
 * telling somebody what computers its owner has.
 */
@Composable
private fun LockScreen(failure: String?, onUnlock: () -> Unit) {
    Column(
        Modifier
            .fillMaxSize()
            .background(MaterialTheme.colorScheme.background)
            .padding(32.dp),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text("APEX Remote", style = MaterialTheme.typography.titleLarge)
        Spacer(Modifier.height(10.dp))
        Text(
            "Unlock to see your computers.",
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        failure?.let {
            Spacer(Modifier.height(16.dp))
            Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
        }
        Spacer(Modifier.height(24.dp))
        Button(
            onClick = onUnlock,
            shape = RoundedCornerShape(6.dp),
            colors = ButtonDefaults.buttonColors(
                containerColor = MaterialTheme.colorScheme.primary,
                contentColor = MaterialTheme.colorScheme.onPrimary,
            ),
        ) { Text("Unlock") }
    }
}
