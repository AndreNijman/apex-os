package com.apexos.remote.ui.agent

import android.content.ActivityNotFoundException
import android.content.Intent
import android.speech.RecognizerIntent
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
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
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
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
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.apexos.remote.core.agent.AgentGraph
import com.apexos.remote.core.agent.AgentSession
import com.apexos.remote.core.agent.AgentStates
import com.apexos.remote.core.agent.Elapsed
import com.apexos.remote.core.agent.Gauge
import com.apexos.remote.core.agent.Handoff
import com.apexos.remote.core.agent.Reply
import com.apexos.remote.core.agent.live
import com.apexos.remote.ui.theme.ApexTones
import com.apexos.remote.ui.theme.MachineText
import com.apexos.remote.ui.theme.labelColumnWidth

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
    /**
     * The most recent `list`, so push-to-talk can resolve its destination
     * through `Reply.check` before opening anything.
     */
    liveSessions: List<AgentSession>,
    nowSeconds: Long,
    busy: String?,
    failure: String?,
    onAttach: () -> Unit,
    onPause: () -> Unit,
    onResume: () -> Unit,
    onInterrupt: () -> Unit,
    onStop: () -> Unit,
    onRefresh: () -> Unit,
    /** Type a line into the session without attaching a terminal. */
    onReply: (String) -> Unit,
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
                            if (!session.live || session.state != AgentStates.WORKING) {
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

            ReplyBox(session, machine, liveSessions, onReply)

            Section("Where it is running")
            Field("Directory", session.cwd)
            session.projectName?.let { Field("Project", it) }
            session.worktree?.let { Field("Worktree", it) }
            session.telemetry?.branch?.let { Field("Branch", it) }

            session.checkpoint?.let { Checkpoint(it) }

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
                            modifier = Modifier.width(labelColumnWidth()),
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
            modifier = Modifier.width(labelColumnWidth()),
        )
        Text(value, style = MachineText, modifier = Modifier.weight(1f))
    }
}

/**
 * Answering an agent that is waiting for you, without opening a terminal.
 *
 * This is P1-058's "input-needed workflow" as this runtime can support it.
 * `Request::Input` writes bytes into the session's PTY, and a phone is allowed
 * it — `privilege::refuse_input` refuses only a caller that IS a session and
 * one whose origin could not be classified, and a paired device is neither.
 * See `Reply.kt`.
 *
 * Offered only while the agent is actually waiting. A box that was always
 * there would invite typing into a working agent, where the bytes queue on the
 * terminal and surface later in the middle of whatever it then asks — which
 * reads, correctly, as the app having sent the reply somewhere random.
 */
@Composable
private fun ReplyBox(
    session: AgentSession,
    machine: String,
    liveSessions: List<AgentSession>,
    onReply: (String) -> Unit,
) {
    if (!session.live || !session.needsYou || session.paused) return
    var text by remember(session.id) { mutableStateOf("") }
    // NOT keyed on the session. The recogniser is another activity and its
    // result comes back to whatever is on screen then, so this state has to
    // outlive a change of session in order for `landsOn` to catch one.
    var voice by remember { mutableStateOf(Handoff.Voice.State()) }
    var choice by remember(session.id) { mutableStateOf(Handoff.Clipboard.Choice.FIRST_LINE) }
    val clipboard = LocalClipboardManager.current
    val onScreen = Reply.Target(machine, session.id, session.started)

    val recogniser = rememberLauncherForActivityResult(
        ActivityResultContracts.StartActivityForResult(),
    ) { result ->
        val heard = result.data
            ?.getStringArrayListExtra(RecognizerIntent.EXTRA_RESULTS)
            ?.firstOrNull()
            .orEmpty()
        // The frozen target. See Handoff.Voice.landsOn: without this, one
        // agent's dictated answer lands in another agent's box, where the
        // person reads it as their own words and presses Send.
        voice = when {
            !Handoff.Voice.landsOn(
                Handoff.Voice.transcribed(Handoff.Voice.stop(voice), heard.ifBlank { "x" }),
                onScreen,
            ) -> Handoff.Voice.failed(voice, Handoff.Voice.LANDED_ELSEWHERE)

            else -> {
                val next = Handoff.Voice.transcribed(Handoff.Voice.stop(voice), heard)
                if (next.phase == Handoff.Voice.Phase.DELIVERING) {
                    // Into the box, unsubmitted, where a person reads it.
                    text = (text.trimEnd() + " " + Handoff.Voice.forReview(next.text)).trimStart()
                    Handoff.Voice.delivered(next)
                } else {
                    next
                }
            }
        }
    }

    Section("Reply")
    Text(
        "Typed straight into the agent's terminal on the machine, the same as if you were " +
            "sitting at it. Sending nothing is a bare return, which is how you accept a " +
            "default.",
        style = MaterialTheme.typography.labelSmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(horizontal = 16.dp),
    )
    Spacer(Modifier.height(8.dp))
    OutlinedTextField(
        value = text,
        onValueChange = { text = it },
        placeholder = { Text("Your answer") },
        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp),
        // Multi-line, because an answer to an agent is often a sentence and a
        // single-line field turns the action key into "send" — which would
        // make an accidental tap of the keyboard's return key an irreversible
        // instruction to a machine somewhere else.
        singleLine = false,
        maxLines = 4,
    )

    // The send-path gate, applied to whatever is in the box HOWEVER IT GOT
    // THERE — typed, dictated, or pasted by the keyboard's own Paste, which
    // runs no code of this app's. On a PTY an interior newline IS the return
    // key, so a pasted stack trace would submit its first line and type the
    // rest into whatever the agent asked next. `Reply.bytes` trims the ends
    // only and does not save you.
    val shape = Handoff.Clipboard.inspect(text)
    if (!shape.isPlain) {
        Spacer(Modifier.height(8.dp))
        Text(
            buildString {
                if (shape.submits) {
                    append(
                        "This is ${shape.lines} lines. On the agent's terminal every line " +
                            "break is a press of Enter, so sending all of it runs the first " +
                            "line and types the rest into whatever it asks next.",
                    )
                }
                if (shape.controlBytes.isNotEmpty()) {
                    if (isNotEmpty()) append(" ")
                    append(
                        "It also carries control characters " +
                            shape.controlBytes.joinToString(", ") { "0x%02x".format(it) } +
                            ", which a terminal acts on rather than shows.",
                    )
                }
                if (shape.truncated) {
                    if (isNotEmpty()) append(" ")
                    append("Only the first ${Handoff.Clipboard.MAX_CHARS} characters would be sent.")
                }
            },
            style = MaterialTheme.typography.labelSmall,
            color = MaterialTheme.colorScheme.error,
            modifier = Modifier.padding(horizontal = 16.dp),
        )
        Spacer(Modifier.height(4.dp))
        Row(Modifier.padding(horizontal = 16.dp)) {
            for (c in Handoff.Clipboard.Choice.entries) {
                val label = when (c) {
                    Handoff.Clipboard.Choice.FIRST_LINE -> "First line only"
                    Handoff.Clipboard.Choice.EVERYTHING -> "Send all of it"
                }
                if (c == choice) {
                    Button(onClick = {}, shape = RoundedCornerShape(6.dp)) { Text(label) }
                } else {
                    OutlinedButton(
                        onClick = { choice = c },
                        shape = RoundedCornerShape(6.dp),
                    ) { Text(label) }
                }
                Spacer(Modifier.width(8.dp))
            }
        }
    }

    voice.error.takeIf { it.isNotBlank() }?.let {
        Spacer(Modifier.height(8.dp))
        Text(
            it,
            style = MaterialTheme.typography.labelSmall,
            color = MaterialTheme.colorScheme.error,
            modifier = Modifier.padding(horizontal = 16.dp),
        )
    }

    Spacer(Modifier.height(8.dp))
    Row(Modifier.padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
        Button(
            onClick = {
                onReply(
                    if (shape.isPlain) text else Handoff.Clipboard.forReview(text, choice),
                )
                text = ""
            },
            shape = RoundedCornerShape(6.dp),
        ) { Text(if (Reply.isBare(text)) "Send a bare return" else "Send") }
        Spacer(Modifier.width(8.dp))
        OutlinedButton(
            onClick = {
                // Resolved BEFORE anything opens, through Reply.check, so the
                // recogniser never opens for a session that has gone.
                val route = Handoff.Voice.route(machine, session, liveSessions)
                voice = Handoff.Voice.start(voice, route, System.currentTimeMillis())
                if (voice.phase == Handoff.Voice.Phase.RECORDING) {
                    val intent = Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH).apply {
                        putExtra(
                            RecognizerIntent.EXTRA_LANGUAGE_MODEL,
                            RecognizerIntent.LANGUAGE_MODEL_FREE_FORM,
                        )
                        putExtra(RecognizerIntent.EXTRA_PROMPT, "Speak to ${route.label}")
                    }
                    // Caught rather than pre-checked with `resolveActivity`,
                    // which on API 30+ answers null for an intent this app has
                    // not declared in <queries> — reporting "no recogniser" on
                    // a phone that has one. The <queries> entry is declared
                    // too; this is the belt.
                    try {
                        recogniser.launch(intent)
                    } catch (e: ActivityNotFoundException) {
                        voice = Handoff.Voice.failed(
                            voice,
                            Handoff.Voice.Refusal.NO_RECOGNISER.message,
                        )
                    }
                }
            },
            shape = RoundedCornerShape(6.dp),
        ) { Text("Speak") }
        Spacer(Modifier.width(8.dp))
        OutlinedButton(
            onClick = {
                // A convenience over the gate above, not a second path: what it
                // puts in the box is inspected by the same `shape` on the next
                // recomposition, exactly as a keyboard paste would be.
                val copied = clipboard.getText()?.text.orEmpty()
                if (copied.isNotEmpty()) text = copied
            },
            shape = RoundedCornerShape(6.dp),
        ) { Text("Paste") }
    }

    Spacer(Modifier.height(8.dp))
    Text(
        Handoff.Files.WHERE,
        style = MaterialTheme.typography.labelSmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(horizontal = 16.dp),
    )
    Spacer(Modifier.height(8.dp))
}

/**
 * The checkpoint this session was started with.
 *
 * A command to read, not a button, and that is measured rather than cautious.
 * `checkpoint::list` and `checkpoint::restore` exist in `apex-agent-core`, but
 * their only non-test callers are in `apexd/apex/src/agent.rs` — the CLI —
 * working on the local filesystem directly. **There is no checkpoint verb on
 * the socket**: not list, not restore, not undo.
 *
 * So P1-056's "checkpoint/undo actions show consequences before execution" is
 * not met here and cannot be. What a restore would change is computed on the
 * machine from the checkpoint's tree, and none of it crosses this wire —
 * summarising the consequences from the fields that ARE here would be
 * describing a destructive operation from the wrong data. What the screen can
 * honestly do is say the checkpoint exists, say what the command is, and say
 * that the machine is where it runs.
 */
@Composable
private fun Checkpoint(id: String) {
    Section("Checkpoint")
    Text(
        "APEX captured the project before this agent started. Undoing is done at the " +
            "machine — there is no checkpoint request on this connection, so this app can " +
            "neither run it nor tell you what it would change.",
        style = MaterialTheme.typography.bodySmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(horizontal = 16.dp),
    )
    Spacer(Modifier.height(6.dp))
    Field("Run there", "apex agent undo --checkpoint $id")
}
