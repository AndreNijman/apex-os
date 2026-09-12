package com.apexos.remote.ui.agent

import androidx.compose.foundation.background
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
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
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
import com.apexos.remote.core.agent.live
import com.apexos.remote.ui.theme.ApexTones
import com.apexos.remote.ui.theme.MachineText

/**
 * One agent, in full: what it is, what it is doing, what it forked, and the
 * four things that can be done to it.
 *
 * ## `pause` and `stop` are signals, and the buttons say so
 *
 * There is no pause verb on this socket. `pause` is `SIGSTOP` and `resume` is
 * `SIGCONT`, `stop` is `SIGTERM` and `interrupt` is `SIGINT` — and `paused`
 * becomes true only after the kill succeeds. The buttons are labelled for what
 * a person wants and the consequence is spelled out underneath, because
 * "Stop" meaning SIGTERM and "Stop" meaning SIGKILL are different promises and
 * only one of them lets the agent clean up.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SessionScreen(
    session: AgentSession,
    machine: String,
    nowSeconds: Long,
    busy: String?,
    failure: String?,
    onAttach: () -> Unit,
    onPause: () -> Unit,
    onResume: () -> Unit,
    onInterrupt: () -> Unit,
    onStop: () -> Unit,
    onRefresh: () -> Unit,
    onBack: () -> Unit,
    onDismiss: () -> Unit,
) {
    val tone = ApexTones.forState(session.state)
    Scaffold(
        containerColor = MaterialTheme.colorScheme.background,
        topBar = {
            TopAppBar(
                title = {
                    Text(
                        "${session.agentName} on $machine",
                        style = MaterialTheme.typography.titleMedium,
                        maxLines = 1,
                    )
                },
                navigationIcon = { TextButton(onClick = onBack) { Text("Back") } },
                actions = { TextButton(onClick = onRefresh) { Text("Refresh") } },
                colors = TopAppBarDefaults.topAppBarColors(
                    containerColor = MaterialTheme.colorScheme.background,
                ),
            )
        },
    ) { padding ->
        Column(
            Modifier
                .fillMaxSize()
                .padding(padding)
                .verticalScroll(rememberScrollState()),
        ) {
            busy?.let { Strip(it, alarming = false) }
            failure?.let { Strip(it, alarming = true, onDismiss = onDismiss) }

            Row(Modifier.padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
                StateBadge(session.state, size = 34.dp)
                Spacer(Modifier.width(14.dp))
                Column {
                    Text(StateLabel(session.state), style = MaterialTheme.typography.titleMedium, color = tone)
                    session.detail?.takeIf { it.isNotBlank() }?.let {
                        Text(
                            it,
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    Text(
                        // Two different facts, and a session waiting two
                        // minutes is not a session waiting two hours.
                        buildString {
                            append("running ").append(Elapsed.of(session, nowSeconds))
                            if (!session.live || session.state != "working") {
                                append(" · quiet ").append(Elapsed.formatMs(session.idleMs(nowSeconds * 1000)))
                            }
                        },
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }

            Controls(
                session = session,
                onAttach = onAttach,
                onPause = onPause,
                onResume = onResume,
                onInterrupt = onInterrupt,
                onStop = onStop,
            )

            Section("Where it is running")
            Field("Directory", session.cwd)
            session.projectName?.let { Field("Project", it) }
            session.worktree?.let { Field("Worktree", it) }
            session.telemetry?.branch?.let { Field("Branch", it) }

            Section("Mode")
            // The policy, flattened into the session by the daemon. Six
            // dimensions; `connectors` is omitted because nothing on a phone
            // shows it. An absent dimension is absent, not "default" — the
            // daemon knows what it configured and this does not.
            PolicyChips(session)
            session.requestOrigin?.let { Field("Requested by", it) }

            Section("What it reported")
            val telemetry = session.telemetry
            if (telemetry?.model == null && telemetry?.contextPct == null) {
                // Said rather than left blank. "Nothing reported" and "nothing
                // to report" look the same on a screen and are not the same.
                Text(
                    "This agent has not reported a model or a context figure. " +
                        "That is not the same as zero.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp),
                )
            } else {
                telemetry.model?.let { Field("Model", it) }
                Gauge.fraction(telemetry.contextPct)?.let { fraction ->
                    Row(
                        Modifier.padding(horizontal = 16.dp, vertical = 6.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text(
                            "Context",
                            style = MaterialTheme.typography.labelSmall,
                            modifier = Modifier.width(110.dp),
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        LinearProgressIndicator(
                            progress = { fraction },
                            color = tone,
                            modifier = Modifier.weight(1f).height(4.dp),
                        )
                        Spacer(Modifier.width(8.dp))
                        Text(Gauge.label(telemetry.contextPct) ?: "", style = MaterialTheme.typography.labelSmall)
                    }
                }
            }
            // The account-wide figures, once, under their own heading —
            // never per row. The daemon's comment: six sessions on one login
            // report the same number.
            if (telemetry?.fiveHourPct != null || telemetry?.sevenDayPct != null) {
                Section("This account's usage")
                Gauge.label(telemetry.fiveHourPct)?.let { Field("Five hours", it) }
                Gauge.label(telemetry.sevenDayPct)?.let { Field("Seven days", it) }
            }

            Section("What it forked")
            Graph(session)

            Spacer(Modifier.height(32.dp))
        }
    }
}

@Composable
private fun Controls(
    session: AgentSession,
    onAttach: () -> Unit,
    onPause: () -> Unit,
    onResume: () -> Unit,
    onInterrupt: () -> Unit,
    onStop: () -> Unit,
) {
    Column(Modifier.padding(horizontal = 16.dp)) {
        Button(
            onClick = onAttach,
            shape = RoundedCornerShape(6.dp),
            modifier = Modifier.fillMaxWidth(),
        ) { Text(if (session.live) "Open the terminal" else "Read the transcript") }
        Spacer(Modifier.height(8.dp))
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            if (session.paused) {
                OutlinedButton(onClick = onResume, enabled = session.live, shape = RoundedCornerShape(6.dp)) {
                    Text("Resume")
                }
            } else {
                OutlinedButton(onClick = onPause, enabled = session.live, shape = RoundedCornerShape(6.dp)) {
                    Text("Pause")
                }
            }
            OutlinedButton(onClick = onInterrupt, enabled = session.live, shape = RoundedCornerShape(6.dp)) {
                Text("Interrupt")
            }
            OutlinedButton(
                onClick = onStop,
                enabled = session.live,
                shape = RoundedCornerShape(6.dp),
                colors = ButtonDefaults.outlinedButtonColors(
                    contentColor = MaterialTheme.colorScheme.error,
                ),
            ) { Text("Stop") }
        }
        Text(
            // The consequences, in one line, because the three buttons are
            // three different signals and only the words say which.
            if (session.paused) {
                "Resume sends SIGCONT. Interrupt sends SIGINT — the same as Ctrl-C. " +
                    "Stop sends SIGTERM, which lets the agent clean up."
            } else {
                "Pause sends SIGSTOP: the agent freezes where it is and does not know it. " +
                    "Interrupt sends SIGINT — the same as Ctrl-C. Stop sends SIGTERM, " +
                    "which lets the agent clean up."
            },
            style = MaterialTheme.typography.labelSmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.padding(top = 8.dp),
        )
        if (!session.live) {
            Text(
                buildString {
                    append("This session has ended")
                    session.exitCode?.let { append(" — exit $it") }
                    session.exitSignal?.let { append(" — killed by signal $it") }
                    append(". Its transcript is still on the machine.")
                },
                style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(top = 6.dp),
            )
        }
    }
}

@Composable
private fun PolicyChips(session: AgentSession) {
    val dimensions = listOfNotNull(
        session.sandbox?.let { "sandbox $it" },
        session.network?.let { "network $it" },
        session.system?.let { "system $it" },
        session.secrets?.let { "secrets $it" },
        session.native?.let { "native $it" },
    )
    if (dimensions.isEmpty()) {
        Text(
            "The machine did not report a policy for this session.",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp),
        )
        return
    }
    Row(
        Modifier.padding(horizontal = 16.dp, vertical = 4.dp).fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        for (d in dimensions.take(3)) Chip(d)
    }
    if (dimensions.size > 3) {
        Row(
            Modifier.padding(horizontal = 16.dp, vertical = 4.dp).fillMaxWidth(),
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            for (d in dimensions.drop(3)) Chip(d)
        }
    }
}

@Composable
private fun Chip(text: String) {
    Surface(
        color = MaterialTheme.colorScheme.surfaceVariant,
        shape = RoundedCornerShape(4.dp),
    ) {
        Text(
            text,
            style = MaterialTheme.typography.labelSmall,
            modifier = Modifier.padding(horizontal = 8.dp, vertical = 4.dp),
        )
    }
}

/**
 * The graph, as indented rows.
 *
 * `AgentGraph` builds the forest and keeps the daemon's order at every level,
 * so nothing appears to move when a sibling exits. A child whose parent has
 * already gone is shown as a root rather than dropped — the daemon's
 * `children` is a live view, and showing fewer processes than exist is the one
 * direction a supervision tool must never be wrong in.
 */
@Composable
private fun Graph(session: AgentSession) {
    val rows = AgentGraph.rows(session.children)
    if (rows.isEmpty()) {
        Text(
            // The distinction the daemon's own comment calls load-bearing.
            "This session has forked nothing.",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp),
        )
        return
    }
    Column {
        for (row in rows) {
            Row(
                Modifier
                    .fillMaxWidth()
                    .padding(start = 16.dp + (row.depth * 16).dp, end = 16.dp, top = 4.dp, bottom = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Box(
                    Modifier
                        .width(6.dp)
                        .height(6.dp)
                        .clip(RoundedCornerShape(3.dp))
                        .background(
                            if (row.info.live) {
                                ApexTones.forToken("info")
                            } else {
                                MaterialTheme.colorScheme.onSurfaceVariant
                            },
                        ),
                )
                Spacer(Modifier.width(8.dp))
                Text(
                    row.info.label.ifEmpty { row.info.id },
                    style = MachineText,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
                Text(
                    buildString {
                        append(if (row.info.isSubagent) "subagent" else "process")
                        row.info.rssKb?.let { append(" · ").append(it / 1024).append(" MB") }
                        if (!row.info.live) append(" · ended")
                    },
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
}

@Composable
private fun Section(title: String) {
    HorizontalDivider(color = MaterialTheme.colorScheme.surfaceVariant, modifier = Modifier.padding(top = 16.dp))
    Text(
        title,
        style = MaterialTheme.typography.labelSmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(start = 16.dp, top = 12.dp, bottom = 4.dp),
    )
}

@Composable
private fun Field(label: String, value: String) {
    Row(Modifier.padding(horizontal = 16.dp, vertical = 3.dp), verticalAlignment = Alignment.Top) {
        Text(
            label,
            style = MaterialTheme.typography.labelSmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.width(110.dp),
        )
        Text(value, style = MachineText, modifier = Modifier.weight(1f))
    }
}
