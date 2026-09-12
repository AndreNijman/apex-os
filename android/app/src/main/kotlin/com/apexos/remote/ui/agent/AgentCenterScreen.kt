package com.apexos.remote.ui.agent

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
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.apexos.remote.core.agent.AgentGraph
import com.apexos.remote.core.agent.AgentSession
import com.apexos.remote.core.agent.Elapsed
import com.apexos.remote.core.agent.Gauge
import com.apexos.remote.core.agent.Order
import com.apexos.remote.core.agent.live
import com.apexos.remote.ui.theme.ApexTones
import com.apexos.remote.ui.theme.MachineText

/**
 * Every agent on one machine, and what each of them is doing.
 *
 * The order is the desktop's and is decided in `:core` — the ones that need
 * you as their own group above everything, then live before finished, then
 * most recently active. Doing it there rather than here is what stops a badge
 * count, a notification and this list disagreeing about which session is
 * first.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AgentCenterScreen(
    machine: String,
    sessions: List<AgentSession>,
    nowSeconds: Long,
    busy: String?,
    failure: String?,
    onRefresh: () -> Unit,
    onOpen: (AgentSession) -> Unit,
    onStart: () -> Unit,
    onBack: () -> Unit,
    onDismiss: () -> Unit,
) {
    val (needsYou, rest) = Order.groups(sessions)

    Scaffold(
        containerColor = MaterialTheme.colorScheme.background,
        topBar = {
            TopAppBar(
                title = { Text(machine, style = MaterialTheme.typography.titleMedium, maxLines = 1) },
                navigationIcon = { TextButton(onClick = onBack) { Text("Back") } },
                actions = { TextButton(onClick = onRefresh) { Text("Refresh") } },
                colors = TopAppBarDefaults.topAppBarColors(
                    containerColor = MaterialTheme.colorScheme.background,
                ),
            )
        },
    ) { padding ->
        Column(Modifier.fillMaxSize().padding(padding)) {
            busy?.let { Strip(it, alarming = false) }
            failure?.let { Strip(it, alarming = true, onDismiss = onDismiss) }

            if (sessions.isEmpty() && busy == null) {
                Empty(onStart)
            } else {
                LazyColumn(Modifier.weight(1f)) {
                    if (needsYou.isNotEmpty()) {
                        item {
                            // A heading, which is why the two are groups rather
                            // than a sort: "two agents are waiting for you" is
                            // legible without counting badges.
                            Heading(
                                if (needsYou.size == 1) {
                                    "1 agent needs you"
                                } else {
                                    "${needsYou.size} agents need you"
                                },
                            )
                        }
                        items(needsYou, key = { "n${it.id}" }) {
                            SessionRow(it, nowSeconds) { onOpen(it) }
                        }
                    }
                    if (rest.isNotEmpty()) {
                        if (needsYou.isNotEmpty()) item { Heading("Everything else") }
                        items(rest, key = { "r${it.id}" }) {
                            SessionRow(it, nowSeconds) { onOpen(it) }
                        }
                    }
                }
                Button(
                    onClick = onStart,
                    shape = RoundedCornerShape(6.dp),
                    modifier = Modifier.fillMaxWidth().padding(16.dp),
                ) { Text("Start an agent") }
            }
        }
    }
}

@Composable
private fun Heading(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.labelSmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 16.dp, bottom = 4.dp),
    )
}

/**
 * One session.
 *
 * The stripe down the left is the reason the page reads as coloured at a
 * glance rather than as a list of white text with one small badge on each
 * line — the desktop's `StateBadge.qml` exposes `toneColor` for exactly this,
 * and says so.
 */
@Composable
fun SessionRow(session: AgentSession, nowSeconds: Long, onClick: () -> Unit) {
    val tone = ApexTones.forState(session.state)
    Row(
        Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .padding(end = 16.dp, top = 10.dp, bottom = 10.dp),
        verticalAlignment = Alignment.Top,
    ) {
        Box(Modifier.width(3.dp).height(52.dp).background(tone))
        Spacer(Modifier.width(13.dp))
        StateBadge(session.state)
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(session.agentName, style = MaterialTheme.typography.titleSmall)
                session.telemetry?.model?.let {
                    // The model, only when the agent has told us. A row that
                    // printed a default would be naming a model nobody chose.
                    Spacer(Modifier.width(6.dp))
                    Text(it, style = MaterialTheme.typography.labelSmall, color = tone)
                }
                Spacer(Modifier.weight(1f))
                Text(
                    Elapsed.of(session, nowSeconds),
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            Text(
                session.where,
                style = MachineText,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.MiddleEllipsis,
            )
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(StateLabel(session.state), style = MaterialTheme.typography.labelSmall, color = tone)
                // The runtime's own words about the state, when it attached
                // any: a step, an error, "paused". Never invented here.
                session.detail?.takeIf { it.isNotBlank() }?.let {
                    Spacer(Modifier.width(6.dp))
                    Text(
                        it,
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
            }
            val branch = session.telemetry?.branch
            val graph = AgentGraph.summary(session.children)
            if (branch != null || graph != null) {
                Text(
                    listOfNotNull(branch, graph).joinToString(" · "),
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            // The context gauge, and ONLY when the agent has reported one.
            // `Telemetry` says it plainly: a gauge drawn at 0% for a session
            // that has never reported is a gauge that is lying.
            Gauge.fraction(session.telemetry?.contextPct)?.let { fraction ->
                Spacer(Modifier.height(4.dp))
                Row(verticalAlignment = Alignment.CenterVertically) {
                    LinearProgressIndicator(
                        progress = { fraction },
                        color = tone,
                        modifier = Modifier.weight(1f).height(3.dp),
                    )
                    Spacer(Modifier.width(6.dp))
                    Text(
                        "context ${Gauge.label(session.telemetry?.contextPct)}",
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
        }
    }
    HorizontalDivider(color = MaterialTheme.colorScheme.surfaceVariant)
}

@Composable
private fun Empty(onStart: () -> Unit) {
    Column(
        Modifier.fillMaxSize().padding(32.dp),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text("Nothing is running", style = MaterialTheme.typography.titleMedium)
        Spacer(Modifier.height(8.dp))
        Text(
            "This machine has no agent sessions. Starting one here runs it on the " +
                "machine, not on this phone — it keeps going when you put the phone away.",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Spacer(Modifier.height(20.dp))
        Button(onClick = onStart, shape = RoundedCornerShape(6.dp)) { Text("Start an agent") }
    }
}

@Composable
internal fun Strip(text: String, alarming: Boolean, onDismiss: (() -> Unit)? = null) {
    Surface(
        color = if (alarming) {
            MaterialTheme.colorScheme.errorContainer
        } else {
            MaterialTheme.colorScheme.surfaceVariant
        },
        modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 6.dp).clip(RoundedCornerShape(6.dp)),
    ) {
        Row(Modifier.padding(12.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(text, style = MaterialTheme.typography.bodySmall, modifier = Modifier.weight(1f))
            onDismiss?.let { TextButton(onClick = it) { Text("OK") } }
        }
    }
}

/** Whether a session can still be signalled. A dead one cannot. */
val AgentSession.controllable: Boolean get() = live
