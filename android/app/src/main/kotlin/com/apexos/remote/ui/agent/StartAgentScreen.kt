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
import androidx.compose.material3.Switch
import androidx.compose.foundation.layout.width
import androidx.compose.ui.Alignment
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.runtime.LaunchedEffect
import com.apexos.remote.core.agent.AgentNames
import com.apexos.remote.core.agent.AgentProfile
import com.apexos.remote.core.agent.Agentd
import com.apexos.remote.core.agent.Hello
import com.apexos.remote.core.agent.ProjectRecord
import com.apexos.remote.ui.PickerUiState
import com.apexos.remote.ui.theme.MachineText

/**
 * Start an agent on the machine.
 *
 * ## P1-054 asks for "profile/project/worktree", and now all three exist
 *
 * The comment that stood here said two of the three were not concepts this
 * socket could be asked about. That was true when it was written and is not
 * any more: `Request::Projects` and `Request::Profiles` are on the wire, and
 * [com.apexos.remote.core.agent.ProjectRecord] carries the account of how that
 * is asserted rather than remembered — the same fixture is parsed by this
 * app's tests and by the daemon's own serde, and `apex-agentd`'s
 * `project_picker.rs` drives both verbs through a real daemon, including from
 * a connection carrying the origin a paired phone's requests arrive under.
 *
 * * **Profile** is the adapter, annotated. There is one profile *description*
 *   in this runtime (`claude`) and six adapters, so the picker stays keyed on
 *   adapters — but each now says whether its program is on THAT machine's
 *   `PATH` and whether its profile directory is there. A phone that offered
 *   `codex` on a machine with no `codex` offered a button whose only outcome
 *   was a refusal after a round trip; a device measured exactly that.
 * * **Project** is a picker, from `projects`. That verb reads one small JSON
 *   record per project and runs no subprocess, which is what makes it usable
 *   on screen open — the reason there was no picker before was `worktrees`'
 *   cost, not the absence of a listing.
 * * **Worktree** is a picker too, but only after a project is chosen, because
 *   it IS `worktrees` and it does run git. It stays a text field as well: the
 *   verb creates a worktree that does not exist yet, so a picker alone would
 *   remove the ability to make a new one.
 *
 * ## What it still falls back to
 *
 * Every machine in the field today predates these two verbs. That answer
 * arrives as an `AgentError` this app can recognise, not as an empty list, and
 * this screen then shows exactly what it showed before — the directories the
 * daemon has already mentioned, and a free-text path — with a line saying the
 * machine is older than the picker. A picker that rendered empty against such
 * a machine would tell the user they have no projects.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun StartAgentScreen(
    machine: String,
    hello: Hello?,
    /** Directories the daemon has already mentioned. May be empty on a fresh machine. */
    knownDirectories: List<String>,
    /** Projects, profiles and the chosen project's worktrees. */
    picker: PickerUiState,
    busy: String?,
    failure: String?,
    /** Ask the machine for projects and profiles. Called once when the screen opens. */
    onLoad: () -> Unit,
    /** Ask for one project's worktrees, by slug. The expensive half. */
    onProject: (slug: String) -> Unit,
    onStart: (
        agent: String?,
        cwd: String,
        worktree: String?,
        prompt: String?,
        checkpoint: Boolean,
        args: List<String>,
    ) -> Unit,
    onBack: () -> Unit,
    onDismiss: () -> Unit,
) {
    var agent by remember(hello) { mutableStateOf(hello?.defaultAgent?.ifEmpty { null }) }
    var cwd by remember(knownDirectories) { mutableStateOf(knownDirectories.firstOrNull() ?: "") }
    var worktree by remember { mutableStateOf("") }
    var command by remember { mutableStateOf("") }
    var checkpoint by remember { mutableStateOf(false) }
    var prompt by remember { mutableStateOf("") }
    var chosenProject by remember { mutableStateOf<String?>(null) }

    // Once per visit, not once per recomposition. `Unit` as the key because
    // the two listings are properties of the machine rather than of anything
    // on this screen, and re-fetching them when the user types a character
    // would spend a round trip per keystroke.
    LaunchedEffect(Unit) { onLoad() }

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
                // What the machine says about the chosen adapter, when it is
                // new enough to have been asked. One line, and only when there
                // is something worth saying: a row that always carries a
                // sentence trains the reader to stop reading it, and then the
                // one that matters — the agent is not on that machine — reads
                // like the rest. `AgentProfile.caution` is where that rule
                // lives, in :core, where a test can reach it.
                val profile = picker.profiles.firstOrNull { it.agent == agent }
                val caution = profile?.caution
                if (caution != null) {
                    Text(
                        caution,
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.error,
                        modifier = Modifier.padding(top = 4.dp),
                    )
                } else if (picker.tooOld) {
                    Text(
                        "That machine's APEX predates the profiles verb, so this app cannot " +
                            "say whether the agent is installed there. Starting one that is " +
                            "not will be refused by the machine.",
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(top = 4.dp),
                    )
                }
            }

            // Only for the adapter that needs one, and it is not decoration:
            // `generic` carries no program, `apex-agentd` refuses a session
            // with none, and this picker offers `generic` because the daemon
            // lists it in `Hello.agents`. Before this field existed, choosing
            // it produced a Start button whose only possible outcome was the
            // daemon's refusal.
            if (Agentd.commandIsRequired(agent, picker.profiles)) {
                Spacer(Modifier.height(16.dp))
                Label("Command")
                OutlinedTextField(
                    value = command,
                    onValueChange = { command = it },
                    singleLine = true,
                    placeholder = { Text("htop") },
                    textStyle = MachineText,
                    isError = command.isBlank(),
                    supportingText = {
                        Text(
                            "The Generic adapter has no program of its own, so it runs the " +
                                "one you name here. Arguments are split on spaces.",
                            style = MaterialTheme.typography.labelSmall,
                        )
                    },
                    modifier = Modifier.fillMaxWidth(),
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
            // The projects the machine remembers. Choosing one sets the
            // directory AND asks for that project's worktrees — the expensive
            // half, deferred to exactly this moment.
            if (picker.projects.isNotEmpty()) {
                LazyRow(
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    modifier = Modifier.padding(top = 8.dp),
                ) {
                    items(picker.projects, key = { it.slug }) { project ->
                        FilterChip(
                            selected = chosenProject == project.slug,
                            onClick = {
                                chosenProject = project.slug
                                cwd = project.root
                                // A worktree name from one project means
                                // nothing in another, and silently carrying it
                                // over would create a tree of that name in the
                                // new project.
                                worktree = ""
                                onProject(project.slug)
                            },
                            label = {
                                Text(
                                    project.label,
                                    maxLines = 1,
                                    overflow = TextOverflow.Ellipsis,
                                )
                            },
                        )
                    }
                }
                val chosen = picker.projects.firstOrNull { it.slug == chosenProject }
                Text(
                    projectDetail(chosen),
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(top = 4.dp),
                )
            } else if (knownDirectories.isNotEmpty()) {
                // The fallback this screen has always had, kept because every
                // machine in the field predates the `projects` verb. The
                // sentence beneath says WHICH of the two reasons applies —
                // "that machine is older" and "nobody has opened a project
                // there" are different facts, and showing one for the other
                // sends the user looking in the wrong place.
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
                    directoryFallbackReason(picker),
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
            // Existing worktrees in the chosen project. A picker BESIDE the
            // text field and not instead of it: `RunRequest.worktree` creates
            // a tree that does not exist yet, so a picker alone would remove
            // the ability to start a new piece of work — which is most of what
            // this field is for.
            //
            // Stamped with the slug it was fetched for, so that choosing a
            // second project while the first project's git is still running
            // cannot show A's worktrees under B.
            val worktreesHere =
                if (picker.worktreesFor != null && picker.worktreesFor == chosenProject) {
                    picker.worktrees
                } else {
                    emptyList()
                }
            if (worktreesHere.isNotEmpty()) {
                LazyRow(
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    modifier = Modifier.padding(top = 8.dp),
                ) {
                    items(worktreesHere, key = { it }) { name ->
                        FilterChip(
                            selected = worktree == name,
                            onClick = { worktree = if (worktree == name) "" else name },
                            label = { Text(name, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                        )
                    }
                }
                Text(
                    "Worktrees that already exist there. Leave it empty to run in the " +
                        "project's own tree, or type a new name to make one.",
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(top = 4.dp),
                )
            }

            Spacer(Modifier.height(16.dp))
            Label("First prompt")
            OutlinedTextField(
                value = prompt,
                onValueChange = { prompt = it },
                placeholder = { Text("optional") },
                minLines = 3,
                modifier = Modifier.fillMaxWidth(),
            )

            Spacer(Modifier.height(16.dp))
            Label("Checkpoint")
            val checkpointLabel = "Capture the project before the agent starts"
            Row(verticalAlignment = Alignment.CenterVertically) {
                Switch(
                    checked = checkpoint,
                    onCheckedChange = { checkpoint = it },
                    // Its words are in the Column BESIDE it, which is a sibling
                    // and not a descendant, so the merged node a screen reader
                    // reads carried no name at all — TalkBack announced this as
                    // "off, switch" and nothing else. Found by walking the
                    // semantics tree on a Pixel 7a; the Accessibility Test
                    // Framework passed the same screen, because an unnamed
                    // switch is not one of the rules ATF has.
                    modifier = Modifier.semantics { contentDescription = checkpointLabel },
                )
                Spacer(Modifier.width(12.dp))
                Column(Modifier.weight(1f)) {
                    Text(checkpointLabel, style = MaterialTheme.typography.bodySmall)
                    // What it does AND what it does not, before it is chosen —
                    // which is as close as this app can get to P1-056's
                    // "checkpoint/undo actions show consequences before
                    // execution". The undo itself is not reachable from here:
                    // there is no checkpoint request on this socket, only this
                    // flag on `run` and the id the daemon reports back.
                    Text(
                        "A commit under refs/apex that is not a branch and is never pushed. " +
                            "Your index, your stash and your branch are left alone. Undoing it " +
                            "is done at the machine with `apex agent undo`; this app can show " +
                            "you the command but cannot run it or say what it would change.",
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }

            Spacer(Modifier.height(20.dp))
            Button(
                onClick = {
                    onStart(
                        agent,
                        cwd.trim(),
                        worktree.trim().ifEmpty { null },
                        prompt.trim().ifEmpty { null },
                        checkpoint,
                        commandArguments(command),
                    )
                },
                enabled = startIsPossible(agent, cwd, command, picker.profiles) && busy == null,
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

/**
 * A command line as the wire wants it: `RunRequest.args`, program first.
 *
 * Split on whitespace and nothing cleverer. There are no quotes and no shell
 * here — `args` goes to `execvp` through a PTY, so a quote a user typed would
 * arrive as a literal quote in an argument, and pretending otherwise would be
 * a shell this app does not have.
 */
fun commandArguments(command: String): List<String> =
    command.trim().split(Regex("\\s+")).filter { it.isNotEmpty() }

/**
 * Whether Start can do anything.
 *
 * A function rather than a boolean expression in the button, because it is the
 * rule this screen exists to get right and a rule in a lambda is a rule no test
 * can reach.
 */
fun startIsPossible(
    agent: String?,
    cwd: String,
    command: String,
    profiles: List<AgentProfile> = emptyList(),
): Boolean =
    cwd.trim().startsWith("/") &&
        !(Agentd.commandIsRequired(agent, profiles) && commandArguments(command).isEmpty()) &&
        // An agent the machine says is not on its PATH. The daemon would
        // refuse this after a round trip, and a button that can only fail is
        // worse than one that is greyed out with the reason beside it.
        //
        // Absent profiles — a machine older than the verb — do NOT disable
        // anything: "this app could not ask" must never read as "the agent is
        // not there".
        profiles.firstOrNull { it.agent == agent }?.programIsPresent != false

/**
 * What to say under the fallback directory chips.
 *
 * Two different facts reach this screen as the same empty list, and telling
 * the user the wrong one sends them looking in the wrong place: a machine
 * older than the `projects` verb has projects it cannot report, and a machine
 * that answered with none genuinely has none opened yet.
 */
fun directoryFallbackReason(picker: PickerUiState): String = when {
    picker.tooOld ->
        "Directories this machine has already run an agent in. Its APEX predates the " +
            "projects verb, so it cannot be asked for the list."
    picker.failure != null ->
        "Directories this machine has already run an agent in. Asking it for its projects " +
            "failed: ${picker.failure}"
    picker.loading || !picker.asked -> "Directories this machine has already run an agent in."
    else ->
        "Directories this machine has already run an agent in. It reports no remembered " +
            "projects — one is remembered the first time an agent runs in it."
}

/**
 * The line under the project chips.
 *
 * Says the WORKSPACE (§8's capsule) when the project has one, because that is
 * the thing P1-056 asks to browse and the thing a user would otherwise have to
 * go to the computer to check. "No workspace" is stated rather than omitted:
 * an absent line reads as "this screen does not know", and the difference
 * matters when the next thing you do is start an agent in it.
 */
fun projectDetail(project: ProjectRecord?): String {
    if (project == null) return "Projects this machine remembers. Choose one to see its worktrees."
    val languages = project.languages.joinToString(", ").ifEmpty { "no toolchain detected" }
    val workspace = project.capsule?.let { "workspace $it" } ?: "no workspace bound"
    return "${project.root} — $languages — $workspace"
}

/** Every directory the daemon has mentioned, newest first, without repeats. */
fun knownDirectories(sessions: List<com.apexos.remote.core.agent.AgentSession>): List<String> =
    sessions
        .sortedByDescending { it.lastActivity }
        .flatMap { listOfNotNull(it.project, it.cwd) }
        .filter { it.startsWith("/") }
        .distinct()
