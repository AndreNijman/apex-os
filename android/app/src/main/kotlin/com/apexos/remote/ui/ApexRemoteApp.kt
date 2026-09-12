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
import com.apexos.remote.ui.agent.AgentCenterScreen
import com.apexos.remote.ui.agent.HelpScreen
import com.apexos.remote.ui.agent.ApprovalsScreen
import com.apexos.remote.ui.agent.SessionScreen
import com.apexos.remote.ui.agent.WorktreesScreen
import com.apexos.remote.ui.agent.StartAgentScreen
import com.apexos.remote.ui.agent.knownDirectories
import com.apexos.remote.ui.term.TerminalScreen
import com.apexos.remote.ui.theme.ApexRemoteTheme
import android.content.Intent
import androidx.compose.ui.platform.LocalContext

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

    /** Every agent on the connected machine. */
    const val AGENTS = "agents"

    /** One agent, in full. */
    const val SESSION = "session"

    /** The attached PTY. */
    const val TERMINAL = "terminal"

    /** Starting a new one. */
    const val START = "start"

    /** Projects, their worktrees and the work in them. */
    const val WORKTREES = "worktrees"

    /** Privileged operations waiting at the machine, and standing grants. */
    const val APPROVALS = "approvals"

    /** The guide (P1-060), whose words live in `:core`. */
    const val HELP = "help"
}

@Composable
fun ApexRemoteApp(
    activity: FragmentActivity,
    deviceName: String,
    /** A payload the app was launched with, from an `apex-remote:` link. */
    launchPayload: String? = null,
    /** Called once the payload has been handed to a screen, so it fires once. */
    onPayloadConsumed: () -> Unit = {},
    /** The (machine, session) a tapped notification named, if this is one. */
    alertTarget: Pair<String, Int>? = null,
    /** Called once the tap has been acted on, so it fires once. */
    onAlertConsumed: () -> Unit = {},
    /** Ask Android for `POST_NOTIFICATIONS`. Owned by the activity. */
    onAskNotifications: () -> Unit = {},
    /** Bumped each time the permission dialog is answered, whichever way. */
    notificationsAnswered: Int = 0,
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
                val context = LocalContext.current
                // Read on arrival. A report written by the handler during the
                // PREVIOUS process is on disk before this one starts, so
                // nothing else would ever notice it.
                LaunchedEffect(state.settings.crashReports) {
                    if (state.settings.crashReports) viewModel.refreshCrash()
                }
                MachinesScreen(
                    state = state,
                    onPair = { navigation.navigate(Destinations.PAIRING) },
                    onConnect = {
                        viewModel.connect(activity, it)
                        navigation.navigate(Destinations.AGENTS)
                    },
                    onPing = { viewModel.ping(activity, it) },
                    onForget = { viewModel.forget(it) },
                    onDynamicColour = { viewModel.setDynamicColour(it) },
                    onCrashReports = { viewModel.setCrashReports(it) },
                    onShareCrash = { report ->
                        // ACTION_SEND with the text in the intent, so the user
                        // picks where it goes. No upload, no reporting SDK,
                        // and no destination this app chose for them.
                        val send = Intent(Intent.ACTION_SEND).apply {
                            type = "text/plain"
                            putExtra(Intent.EXTRA_SUBJECT, "APEX Remote crash report")
                            putExtra(Intent.EXTRA_TEXT, report)
                        }
                        runCatching { context.startActivity(Intent.createChooser(send, null)) }
                    },
                    onClearCrash = { viewModel.clearCrash() },
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
                    // Cleared by the screen once it has captured the code, and
                    // NOT here. `navigate` does not compose anything; it writes
                    // state the `NavHost` reads on the next recomposition. A
                    // consume beside the navigate would therefore run first,
                    // and `PairingScreen` would `remember` an empty string —
                    // which is the exact bug this parameter was added to fix.
                    onPayloadConsumed = onPayloadConsumed,
                    onPayload = { payload ->
                        viewModel.pair(activity, payload, deviceName)
                    },
                    onBack = { navigation.popBackStack() },
                    onDismiss = { viewModel.dismiss() },
                )
            }

            composable(Destinations.AGENTS) {
                // Re-read on arrival AND after the dialog is answered. The
                // second key is what makes this correct: the permission dialog
                // is asynchronous, so a refresh on the line after `launch()`
                // would read the state before the user had answered and
                // conclude they had said no. It also covers the user revoking
                // the permission in Android's settings and coming back.
                LaunchedEffect(notificationsAnswered) { viewModel.refreshNotificationState() }

                // Asked when the user first looks at a machine's agents —
                // the first moment a notification would have anything to say —
                // and not at launch, where it is a dialog in front of somebody
                // who has not seen the app yet. Keyed on the flag rather than
                // on `Unit`, so it fires once the refresh above has actually
                // landed rather than against a stale snapshot.
                LaunchedEffect(state.notificationsUnasked) {
                    if (state.notificationsUnasked) onAskNotifications()
                }
                AgentCenterScreen(
                    machine = state.agents.machine?.machine ?: "",
                    sessions = state.agents.sessions,
                    nowSeconds = state.agents.nowSeconds,
                    busy = state.busy ?: state.agents.busy,
                    failure = state.failure ?: state.agents.failure,
                    onRefresh = { viewModel.refreshAgents() },
                    onOpen = {
                        viewModel.selectSession(it)
                        navigation.navigate(Destinations.SESSION)
                    },
                    onStart = { navigation.navigate(Destinations.START) },
                    onProjects = { navigation.navigate(Destinations.WORKTREES) },
                    onApprovals = { navigation.navigate(Destinations.APPROVALS) },
                    onHelp = { navigation.navigate(Destinations.HELP) },
                    pendingApprovals = state.agents.approvals.pending.size,
                    notificationsEnabled = state.notificationsEnabled,
                    notificationsUnasked = state.notificationsUnasked,
                    onBack = {
                        // Stop the four-second poll. Leaving it running would
                        // keep a control round trip going to a machine nobody
                        // is looking at, for as long as the app is open.
                        viewModel.leaveAgents()
                        navigation.popBackStack()
                    },
                    onDismiss = { viewModel.dismiss() },
                )
            }

            composable(Destinations.SESSION) {
                // Read from the state rather than passed as an argument, so a
                // poll that arrives while this screen is open updates it. A
                // detail screen holding a copy from four seconds ago is one
                // that shows `working` for a session that has since failed.
                val session = state.agents.selected
                if (session == null) {
                    LaunchedEffect(Unit) { navigation.popBackStack() }
                } else {
                    SessionScreen(
                        session = session,
                        machine = state.agents.machine?.machine ?: "",
                        liveSessions = state.agents.sessions,
                        nowSeconds = state.agents.nowSeconds,
                        busy = state.agents.busy,
                        failure = state.agents.failure,
                        onAttach = {
                            if (viewModel.attach(session) != null) {
                                navigation.navigate(Destinations.TERMINAL)
                            }
                        },
                        onPause = { viewModel.signal(session, "stop") },
                        onResume = { viewModel.signal(session, "cont") },
                        onInterrupt = { viewModel.signal(session, "int") },
                        onStop = { viewModel.signal(session, "term") },
                        onRefresh = { viewModel.refreshAgents() },
                        onReply = { viewModel.replyToSession(session, it) },
                        onBack = { navigation.popBackStack() },
                        onDismiss = { viewModel.dismiss() },
                    )
                }
            }

            composable(Destinations.TERMINAL) {
                val controller = viewModel.terminal
                if (controller == null) {
                    LaunchedEffect(Unit) { navigation.popBackStack() }
                } else {
                    TerminalScreen(
                        controller = controller,
                        title = state.agents.selected?.let { "${it.agentName} · ${it.where.substringAfterLast('/')}" }
                            ?: (state.agents.machine?.machine ?: "Terminal"),
                        settings = state.settings,
                        onBack = {
                            // Detaching leaves the session running on the
                            // machine, which is the entire point of a viewport.
                            viewModel.detach()
                            navigation.popBackStack()
                        },
                    )
                }
            }

            composable(Destinations.WORKTREES) {
                WorktreesScreen(
                    machine = state.agents.machine?.deviceId ?: "",
                    state = state.agents.worktrees,
                    // Which sessions this phone actually has, so a worktree
                    // row knows whether "open agent" can do anything. The
                    // daemon's worktree rows carry session ids it knows about,
                    // and the two lists can disagree.
                    liveSessions = state.agents.sessions.map { it.id }.toSet(),
                    onLoad = { viewModel.loadWorktrees() },
                    onOpenSession = {
                        if (viewModel.openWorktreeSession(it) != null) {
                            navigation.navigate(Destinations.SESSION)
                        }
                    },
                    onBack = { navigation.popBackStack() },
                )
            }

            composable(Destinations.APPROVALS) {
                ApprovalsScreen(
                    state = state.agents.approvals,
                    nowSeconds = state.agents.nowSeconds,
                    onLoad = { viewModel.loadApprovals() },
                    onRevokeGrant = { project, key -> viewModel.revokeGrant(project, key) },
                    onRevokeSystemGrant = { viewModel.revokeSystemGrant(it) },
                    onBack = { navigation.popBackStack() },
                )
            }

            composable(Destinations.HELP) {
                HelpScreen(onBack = { navigation.popBackStack() })
            }

            composable(Destinations.START) {
                StartAgentScreen(
                    machine = state.agents.machine?.machine ?: "",
                    hello = state.agents.hello,
                    knownDirectories = knownDirectories(state.agents.sessions),
                    busy = state.agents.busy,
                    failure = state.agents.failure,
                    onStart = { agent, cwd, worktree, prompt, checkpoint ->
                        viewModel.startAgent(
                            cwd = cwd,
                            agent = agent,
                            worktree = worktree,
                            prompt = prompt,
                            checkpoint = checkpoint,
                        )
                    },
                    onBack = { navigation.popBackStack() },
                    onDismiss = { viewModel.dismiss() },
                )
            }
        }

        // A start that worked lands on the new agent's page, which is where
        // the next thing somebody wants to do is. Keyed on the id so it fires
        // once per started session rather than once per recomposition.
        LaunchedEffect(state.agents.selected?.id) {
            if (state.agents.selected != null &&
                navigation.currentDestination?.route == Destinations.START
            ) {
                navigation.popBackStack()
                navigation.navigate(Destinations.SESSION)
            }
        }

        // A tapped notification opens the exact session it named, when this
        // phone still has it. Inside the graph for the same reason as the
        // pairing effect below: `navigate` before the `NavHost` has composed
        // throws, and a cold launch from a notification is exactly that
        // moment.
        LaunchedEffect(alertTarget) {
            val target = alertTarget ?: return@LaunchedEffect
            // Consumed whatever happens. A tap that could not be honoured must
            // not be retried on the next recomposition: the session is not
            // coming back, and the user would be unable to navigate away.
            onAlertConsumed()
            val (machine, session) = target
            if (viewModel.openAlerted(machine, session)) {
                navigation.navigate(Destinations.SESSION) { launchSingleTop = true }
            } else {
                // The session is gone, or it belongs to a machine this phone
                // is not connected to. `SessionInfo.id` is reused after a
                // prune, so landing on whatever now holds that number would be
                // opening a stranger's agent — the Agent Center is the honest
                // destination, and it is where the user can see what IS there.
                navigation.navigate(
                    if (state.agents.machine != null) Destinations.AGENTS else Destinations.MACHINES,
                ) { launchSingleTop = true }
            }
        }

        // A link that carries a pairing code opens the screen that can explain
        // what accepting it would mean. It is deliberately NOT paired on
        // arrival: a URL that paired a phone by being opened would be a URL
        // worth sending somebody. Inside the graph, because navigating before
        // the `NavHost` has composed throws.
        LaunchedEffect(launchPayload) {
            if (launchPayload != null) {
                // `launchSingleTop`, because `onNewIntent` delivers a second
                // link while this screen is already open — the activity is
                // `singleTask` — and without it the second one stacks a
                // duplicate pairing screen on top of the first.
                navigation.navigate(Destinations.PAIRING) { launchSingleTop = true }
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
