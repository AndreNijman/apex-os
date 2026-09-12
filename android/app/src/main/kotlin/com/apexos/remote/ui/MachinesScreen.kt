package com.apexos.remote.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.apexos.remote.core.PairedMachine
import com.apexos.remote.ui.theme.MachineText
import java.text.DateFormat
import java.util.Date

/**
 * The list of computers this phone is paired with, which is the app's home.
 *
 * P1-053 asks for "multiple APEX computers", so a list is the front page rather
 * than a detail somewhere — an app whose home screen is one machine teaches
 * people it holds one machine. Each row shows the machine's name, the device id
 * the *desktop* displays for this phone, and when the pairing was made; those
 * three together are what lets somebody match a row here against a row in
 * `apex remote devices` on the other end, which is the only way to be sure the
 * thing being revoked is the thing they meant.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun MachinesScreen(
    state: UiState,
    onPair: () -> Unit,
    onConnect: (PairedMachine) -> Unit,
    onForget: (PairedMachine) -> Unit,
    onDynamicColour: (Boolean) -> Unit,
    onLock: () -> Unit,
    onDismiss: () -> Unit,
) {
    var forgetting by remember { mutableStateOf<PairedMachine?>(null) }
    var settingsOpen by remember { mutableStateOf(false) }

    Scaffold(
        containerColor = MaterialTheme.colorScheme.background,
        topBar = {
            TopAppBar(
                title = { Text("APEX Remote", style = MaterialTheme.typography.titleMedium) },
                actions = {
                    TextButton(onClick = { settingsOpen = true }) { Text("Display") }
                    TextButton(onClick = onLock) { Text("Lock") }
                },
                colors = TopAppBarDefaults.topAppBarColors(
                    containerColor = MaterialTheme.colorScheme.background,
                ),
            )
        },
    ) { padding ->
        Column(
            Modifier
                .fillMaxSize()
                .background(MaterialTheme.colorScheme.background)
                .padding(padding),
        ) {
            state.gateWarning?.let { Notice(it, alarming = true) }
            state.busy?.let { Notice(it, alarming = false) }
            state.failure?.let { Notice(it, alarming = true, onDismiss = onDismiss) }
            state.message?.let { Notice(it, alarming = false, onDismiss = onDismiss) }
            state.connection?.let {
                Notice(
                    "Connected to ${it.machine}" +
                        (it.roundTripMs?.let { ms -> " — round trip ${ms}ms" } ?: ""),
                    alarming = false,
                    onDismiss = onDismiss,
                )
            }

            if (state.machines.isEmpty() && !state.loading) {
                Empty(onPair)
            } else {
                LazyColumn(Modifier.weight(1f)) {
                    items(state.machines, key = { it.deviceId }) { machine ->
                        MachineRow(
                            machine = machine,
                            onClick = { onConnect(machine) },
                            onForget = { forgetting = machine },
                        )
                    }
                }
                Button(
                    onClick = onPair,
                    shape = RoundedCornerShape(6.dp),
                    colors = ButtonDefaults.buttonColors(
                        containerColor = MaterialTheme.colorScheme.primary,
                        contentColor = MaterialTheme.colorScheme.onPrimary,
                    ),
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(20.dp),
                ) { Text("Add a computer") }
            }
        }
    }

    forgetting?.let { machine ->
        AlertDialog(
            onDismissRequest = { forgetting = null },
            title = { Text("Forget ${machine.machine}?") },
            text = {
                // Both halves, because only one of them is this phone's to do.
                Text(
                    "This phone will delete the key it uses with ${machine.machine}, and the " +
                        "stored pairing becomes unreadable. It does not tell ${machine.machine} " +
                        "anything — to revoke this phone there, run `apex remote revoke " +
                        "${machine.deviceId}` on the machine itself.",
                )
            },
            confirmButton = {
                TextButton(onClick = {
                    onForget(machine)
                    forgetting = null
                }) { Text("Forget", color = MaterialTheme.colorScheme.error) }
            },
            dismissButton = {
                TextButton(onClick = { forgetting = null }) { Text("Keep") }
            },
        )
    }

    if (settingsOpen) {
        AlertDialog(
            onDismissRequest = { settingsOpen = false },
            title = { Text("Display") },
            text = {
                Column {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f)) {
                            Text("Use this phone's colours")
                            Text(
                                "Material You instead of APEX's own palette.",
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                        Switch(
                            checked = state.settings.dynamicColour,
                            onCheckedChange = onDynamicColour,
                        )
                    }
                    Spacer(Modifier.height(12.dp))
                    Text(
                        "Light and dark follow the system setting.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            },
            confirmButton = { TextButton(onClick = { settingsOpen = false }) { Text("Done") } },
        )
    }
}

@Composable
private fun MachineRow(machine: PairedMachine, onClick: () -> Unit, onForget: () -> Unit) {
    Column {
        Row(
            Modifier
                .fillMaxWidth()
                .clickable(onClick = onClick)
                .padding(horizontal = 20.dp, vertical = 14.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(
                Modifier
                    .size(10.dp)
                    .clip(CircleShape)
                    .background(MaterialTheme.colorScheme.primary),
            )
            Spacer(Modifier.width(14.dp))
            Column(Modifier.weight(1f)) {
                Text(
                    machine.machine,
                    style = MaterialTheme.typography.bodyLarge,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                // Monospace, because this is the string somebody compares
                // character by character against `apex remote devices`.
                Text(
                    machine.deviceId,
                    style = MachineText,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Text(
                    "paired ${DateFormat.getDateInstance().format(Date(machine.pairedMs))}" +
                        if (machine.relay != null) " · relay available" else " · LAN only",
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            TextButton(onClick = onForget) {
                Text("Forget", color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
        HorizontalDivider(
            Modifier.padding(start = 44.dp),
            thickness = 1.dp,
            color = MaterialTheme.colorScheme.outline.copy(alpha = 0.4f),
        )
    }
}

@Composable
private fun Empty(onPair: () -> Unit) {
    Column(
        Modifier
            .fillMaxSize()
            .padding(32.dp),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text("No computers yet", style = MaterialTheme.typography.titleLarge)
        Spacer(Modifier.height(10.dp))
        Text(
            "On the machine you want to reach, run `apex remote pair`. It prints a QR code " +
                "that stands for three minutes.",
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Spacer(Modifier.height(24.dp))
        OutlinedButton(onClick = onPair, shape = RoundedCornerShape(6.dp)) {
            Text("Scan the code")
        }
    }
}

/** A line of what just happened, in the colour that says how to feel about it. */
@Composable
fun Notice(text: String, alarming: Boolean, onDismiss: (() -> Unit)? = null) {
    Row(
        Modifier
            .fillMaxWidth()
            .background(
                if (alarming) {
                    MaterialTheme.colorScheme.error.copy(alpha = 0.12f)
                } else {
                    MaterialTheme.colorScheme.primary.copy(alpha = 0.12f)
                },
            )
            .padding(horizontal = 20.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            text,
            style = MaterialTheme.typography.bodySmall,
            color = if (alarming) {
                MaterialTheme.colorScheme.error
            } else {
                MaterialTheme.colorScheme.onBackground
            },
            modifier = Modifier.weight(1f),
        )
        if (onDismiss != null) {
            TextButton(onClick = onDismiss) { Text("OK") }
        }
    }
}
