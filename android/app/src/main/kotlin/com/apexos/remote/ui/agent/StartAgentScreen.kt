package com.apexos.remote.ui.agent

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.apexos.remote.core.agent.AgentNames
import com.apexos.remote.core.agent.Hello
import com.apexos.remote.ui.theme.MachineText

/**
 * Start an agent on the machine.
 *
 * ## What this offers, and why it is three things and not five
 *
 * P1-054 asks for "profile/project/worktree". Two of those three do not exist
 * as concepts this socket can be asked about:
 *
 * * **There is no profile verb.** `apex-agent-core/src/profile.rs` exists and
 *   is not reachable through `Request` at all. The nearest real thing is the
 *   adapter — `Hello.agents` is the list of adapters this runtime has — so
 *   that is what is offered, under its own name rather than dressed up as a
 *   profile.
 * * **There is no projects verb.** There *is* a `worktrees` verb — the
 *   comment that used to stand here said there was not, and it was wrong; see
 *   [com.apexos.remote.core.agent.WorktreeStatus] for what it actually is and
 *   how the mistake was made. It answers with every remembered project's
 *   worktrees, which is where the Worktrees screen gets its listing. It is
 *   deliberately NOT what this screen offers, for a reason that survives the
 *   correction: it is answered by running git in every remembered project,
 *   including `merge-tree --write-tree`, and making that the cost of opening
 *   a "start an agent" form would put seconds of git between a tap and a
 *   text field.
 *
 * So the directories offered here are the ones the daemon has already
 * mentioned — every `cwd` and `project` of every session it reports — with a
 * text field for anything else. `RunRequest.worktree` means "create or reuse
 * this git worktree under the project", so that is a free-text name and is
 * honestly labelled as one.
 *
 * Saying that plainly is better than inventing a picker the runtime will not
 * accept.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun StartAgentScreen(
    machine: String,
    hello: Hello?,
    /** Directories the daemon has already mentioned. May be empty on a fresh machine. */
    knownDirectories: List<String>,
    busy: String?,
    failure: String?,
    onStart: (agent: String?, cwd: String, worktree: String?, prompt: String?) -> Unit,
    onBack: () -> Unit,
    onDismiss: () -> Unit,
) {
    var agent by remember(hello) { mutableStateOf(hello?.defaultAgent?.ifEmpty { null }) }
    var cwd by remember(knownDirectories) { mutableStateOf(knownDirectories.firstOrNull() ?: "") }
    var worktree by remember { mutableStateOf("") }
    var prompt by remember { mutableStateOf("") }

    Scaffold(
        containerColor = MaterialTheme.colorScheme.background,
        topBar = {
            TopAppBar(
                title = { Text("Start an agent", style = MaterialTheme.typography.titleMedium) },
                navigationIcon = { TextButton(onClick = onBack) { Text("Cancel") } },
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
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 16.dp),
        ) {
            busy?.let { Strip(it, alarming = false) }
            failure?.let { Strip(it, alarming = true, onDismiss = onDismiss) }

            Text(
                "It runs on $machine, not on this phone, and keeps going when you put " +
                    "the phone away.",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(vertical = 12.dp),
            )

            Label("Adapter")
            val adapters = hello?.agents.orEmpty()
            if (adapters.isEmpty()) {
                Text(
                    // Not a picker with a guess in it. The machine has not
                    // said which adapters it has, so the default is used and
                    // that fact is on the screen.
                    "The machine has not said which adapters it has. Its own default will be used.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            } else {
                LazyRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    items(adapters, key = { it }) { id ->
                        FilterChip(
                            selected = agent == id,
                            onClick = { agent = if (agent == id) null else id },
                            label = { Text(AgentNames.of(id)) },
                        )
                    }
                }
                Text(
                    // Named for what it is. There is no profile verb on this
                    // socket, and calling an adapter a profile would be
                    // promising something the runtime does not have.
                    "An adapter, not a profile — this runtime has no profiles to pick from.",
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(top = 4.dp),
                )
            }

            Spacer(Modifier.height(16.dp))
            Label("Directory")
            OutlinedTextField(
                value = cwd,
                onValueChange = { cwd = it },
                singleLine = true,
                placeholder = { Text("/home/you/project") },
                textStyle = MachineText,
                isError = cwd.isNotEmpty() && !cwd.startsWith("/"),
                supportingText = {
                    Text(
                        if (cwd.isNotEmpty() && !cwd.startsWith("/")) {
                            "It must be an absolute path: the daemon refuses anything else."
                        } else {
                            "Where the agent runs."
                        },
                        style = MaterialTheme.typography.labelSmall,
                    )
                },
                modifier = Modifier.fillMaxWidth(),
            )
            if (knownDirectories.isNotEmpty()) {
                LazyRow(
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    modifier = Modifier.padding(top = 8.dp),
                ) {
                    items(knownDirectories, key = { it }) { path ->
                        FilterChip(
                            selected = cwd == path,
                            onClick = { cwd = path },
                            label = {
                                Text(
                                    path.substringAfterLast('/').ifEmpty { path },
                                    maxLines = 1,
                                    overflow = TextOverflow.Ellipsis,
                                )
                            },
                        )
                    }
                }
                Text(
                    "Directories this machine has already run an agent in — there is no " +
                        "verb to ask it for a list of projects.",
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(top = 4.dp),
                )
            }

            Spacer(Modifier.height(16.dp))
            Label("Worktree")
            OutlinedTextField(
                value = worktree,
                onValueChange = { worktree = it },
                singleLine = true,
                placeholder = { Text("optional") },
                textStyle = MachineText,
                supportingText = {
                    Text(
                        "A name. The machine creates or reuses a git worktree under the " +
                            "project and runs there.",
                        style = MaterialTheme.typography.labelSmall,
                    )
                },
                modifier = Modifier.fillMaxWidth(),
            )

            Spacer(Modifier.height(16.dp))
            Label("First prompt")
            OutlinedTextField(
                value = prompt,
                onValueChange = { prompt = it },
                placeholder = { Text("optional") },
                minLines = 3,
                modifier = Modifier.fillMaxWidth(),
            )

            Spacer(Modifier.height(20.dp))
            Button(
                onClick = {
                    onStart(
                        agent,
                        cwd.trim(),
                        worktree.trim().ifEmpty { null },
                        prompt.trim().ifEmpty { null },
                    )
                },
                enabled = cwd.trim().startsWith("/") && busy == null,
                shape = RoundedCornerShape(6.dp),
                modifier = Modifier.fillMaxWidth(),
            ) { Text("Start") }
            Spacer(Modifier.height(32.dp))
        }
    }
}

@Composable
private fun Label(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.labelSmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(bottom = 4.dp),
    )
}

/** Every directory the daemon has mentioned, newest first, without repeats. */
fun knownDirectories(sessions: List<com.apexos.remote.core.agent.AgentSession>): List<String> =
    sessions
        .sortedByDescending { it.lastActivity }
        .flatMap { listOfNotNull(it.project, it.cwd) }
        .filter { it.startsWith("/") }
        .distinct()
