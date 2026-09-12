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
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.apexos.remote.core.agent.Elapsed
import com.apexos.remote.core.agent.GrantState
import com.apexos.remote.core.agent.PrivilegeRequest
import com.apexos.remote.core.agent.SystemGrant
import com.apexos.remote.ui.ApprovalsUiState
import com.apexos.remote.ui.theme.ApexTones
import com.apexos.remote.ui.theme.MachineText

/**
 * Privileged operations waiting at the machine, and the authority already
 * standing (P1-057).
 *
 * ## There is no Approve button and there must never be one
 *
 * `apex-agentd/src/privilege.rs:1176` refuses `decide` from any non-local
 * origin **unconditionally**, and the check sits before the pending check — so
 * a paired phone cannot approve and cannot even deny. `Request::Decide`
 * carries no second factor and `decide` never consults `OriginPolicy`; the
 * `remote_elevation_allowed` opt-in gates a different path entirely.
 * `apex-remoted` declares the origin `claude-remote-control` (`proxy.rs:128`)
 * and `origin::may_declare` forbids narrowing back towards local. Pinned
 * upstream by `apex-agentd/tests/request_origin.rs:369`.
 *
 * And every verb in the vocabulary is root: `request.rs:290` maps all eight to
 * `Capability::RootCapability`, deliberately, because the vocabulary was
 * chosen to be exactly `apex`'s root-only subcommands. **So there is no
 * non-root approvable object on this socket at all.**
 *
 * That is why P1-057's first two criteria are not met and cannot be met from
 * this screen. `GitHubPush`, `CloudflarePreviewDeploy` and `ProductionDeploy`
 * exist only as rows in §7's table (`origin.rs:842`) with no serde derive —
 * `Capability` never appears on the wire, and nothing in this build performs
 * them. Production-deploy approval does exist, on `apex-secretd`, and
 * `apex-remoted` never connects to it: no secretd socket path appears anywhere
 * in its sources.
 *
 * What this screen is, then, is the honest thing: it tells you something needs
 * you and where to go. That is worth having — the alternative is finding out
 * when you next sit down — and it is not worth dressing up as approval.
 *
 * ## Revoke IS allowed, and the asymmetry is the point
 *
 * `privilege.rs:1289` applies no origin check to `revoke`, because revocation
 * only ever removes authority. A device that cannot grant anything can still
 * take it back — which is exactly what you want from a phone when you realise
 * from a bus that an agent has a standing grant it should not.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ApprovalsScreen(
    state: ApprovalsUiState,
    nowSeconds: Long,
    onLoad: () -> Unit,
    onRevokeGrant: (project: String, key: String?) -> Unit,
    onRevokeSystemGrant: (Int) -> Unit,
    onBack: () -> Unit,
) {
    LaunchedEffect(Unit) { onLoad() }

    // Revoking is the one action on this screen that removes something, and it
    // is not retried on a dropped connection, so it is confirmed first.
    var confirming by remember { mutableStateOf<Revocation?>(null) }

    Scaffold(
        containerColor = MaterialTheme.colorScheme.background,
        topBar = {
            TopAppBar(
                title = { Text("Approvals", style = MaterialTheme.typography.titleMedium) },
                navigationIcon = { TextButton(onClick = onBack) { Text("Back") } },
                actions = {
                    TextButton(onClick = onLoad, enabled = !state.loading) {
                        Text(if (state.loading) "Loading…" else "Refresh")
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(
                    containerColor = MaterialTheme.colorScheme.background,
                ),
            )
        },
    ) { padding ->
        Column(Modifier.fillMaxSize().padding(padding)) {
            state.busy?.let { Strip(it, alarming = false) }
            state.failure?.let { Strip(it, alarming = true) }

            LazyColumn(Modifier.weight(1f)) {
                item { LocalOnlyNotice() }

                if (state.pending.isNotEmpty()) {
                    item {
                        Heading(
                            if (state.pending.size == 1) {
                                "1 operation is waiting at the machine"
                            } else {
                                "${state.pending.size} operations are waiting at the machine"
                            },
                        )
                    }
                    for (r in state.pending) {
                        item(key = "p${r.id}") { RequestRow(r, nowSeconds) }
                    }
                }

                val decided = state.requests.filterNot { it.isPending }
                if (decided.isNotEmpty()) {
                    item { Heading("Already decided") }
                    for (r in decided.take(30)) {
                        item(key = "d${r.id}") { RequestRow(r, nowSeconds) }
                    }
                }

                if (state.grants.isNotEmpty()) {
                    item { Heading("Standing permissions, by project") }
                    for ((project, keys) in state.grants) {
                        item(key = "g$project") {
                            GrantRow(project, keys) { key -> confirming = Revocation.Project(project, key) }
                        }
                    }
                }

                val open = state.systemGrants.filter { it.first.isOpen }
                if (open.isNotEmpty()) {
                    item { Heading("Live system access") }
                    for ((grant, grantState) in open) {
                        item(key = "s${grant.id}") {
                            SystemGrantRow(grant, grantState) { confirming = Revocation.System(grant.id) }
                        }
                    }
                }

                if (state.requests.isEmpty() && state.grants.isEmpty() && open.isEmpty() &&
                    !state.loading && state.failure == null
                ) {
                    item { Nothing() }
                }
            }
        }
    }

    confirming?.let { target ->
        AlertDialog(
            onDismissRequest = { confirming = null },
            title = { Text(target.title) },
            text = { Text(target.explanation) },
            confirmButton = {
                TextButton(onClick = {
                    when (target) {
                        is Revocation.System -> onRevokeSystemGrant(target.id)
                        is Revocation.Project -> onRevokeGrant(target.project, target.key)
                    }
                    confirming = null
                }) { Text("Revoke") }
            },
            dismissButton = { TextButton(onClick = { confirming = null }) { Text("Keep it") } },
        )
    }
}

/** What a confirmed revoke would do. */
internal sealed interface Revocation {
    val title: String
    val explanation: String

    data class Project(val project: String, val key: String?) : Revocation {
        override val title: String get() = if (key == null) "Revoke every permission?" else "Revoke $key?"
        override val explanation: String
            get() = if (key == null) {
                "Every standing permission for $project is withdrawn. Agents working there " +
                    "will have to ask again, at the machine, for each operation.\n\n" +
                    "This is not retried if the connection drops, so if it fails you will be " +
                    "told it may not have arrived rather than have it sent twice."
            } else {
                "Agents working in $project will have to ask again, at the machine, before " +
                    "running $key.\n\nNot retried if the connection drops."
            }
    }

    data class System(val id: Int) : Revocation {
        override val title: String get() = "End this system access now?"
        override val explanation: String
            get() = "The session loses root access immediately rather than when the window " +
                "runs out. Anything it is part-way through doing that needs root will fail.\n\n" +
                "Not retried if the connection drops."
    }
}

/**
 * The sentence that stands where Allow and Deny would be.
 *
 * It quotes the daemon's own refusal rather than paraphrasing it, so the words
 * the user reads here are the words they would get if the button existed and
 * they pressed it.
 */
@Composable
private fun LocalOnlyNotice() {
    Surface(
        color = MaterialTheme.colorScheme.surfaceVariant,
        modifier = Modifier.fillMaxWidth().padding(12.dp).clip(RoundedCornerShape(6.dp)),
    ) {
        Column(Modifier.padding(12.dp)) {
            Text("You cannot approve from here", style = MaterialTheme.typography.titleSmall)
            Spacer(Modifier.height(4.dp))
            Text(
                PrivilegeRequest.DECIDE_IS_LOCAL_ONLY,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Spacer(Modifier.height(6.dp))
            Text(
                "Withdrawing a permission is allowed, because taking authority away is not " +
                    "granting any.",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

@Composable
private fun RequestRow(request: PrivilegeRequest, nowSeconds: Long) {
    val tone = when {
        request.isPending -> ApexTones.forState("permission_request")
        request.isDenied -> ApexTones.forState("failed")
        else -> ApexTones.forState("complete")
    }
    Row(Modifier.fillMaxWidth().padding(end = 16.dp, top = 10.dp, bottom = 10.dp)) {
        Box(Modifier.width(3.dp).height(52.dp).background(tone))
        Spacer(Modifier.width(13.dp))
        Column(Modifier.weight(1f)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(request.verbLabel, style = MaterialTheme.typography.titleSmall)
                Spacer(Modifier.weight(1f))
                Text(
                    Elapsed.format(maxOf(0, nowSeconds - request.createdMs / 1000)),
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            // The operation in words, including its packages. Root actions are
            // not a verb name to squint at.
            Text("Would ${request.effect}.", style = MaterialTheme.typography.bodySmall)
            request.reason.takeIf { it.isNotBlank() }?.let {
                Text(
                    "\"$it\"",
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 3,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            Text(
                listOfNotNull(
                    request.agent,
                    request.project,
                    request.requestOrigin?.let { origin ->
                        // The origin, and how it was arrived at. `declared` and
                        // `observed` are not the same claim and §7 treats them
                        // differently, so the screen does not flatten them.
                        origin + (request.originSource?.let { " ($it)" } ?: "")
                    },
                ).joinToString(" · "),
                style = MachineText,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.MiddleEllipsis,
            )
            Text(decisionLine(request), style = MaterialTheme.typography.labelSmall, color = tone)
        }
    }
    HorizontalDivider(color = MaterialTheme.colorScheme.surfaceVariant)
}

/**
 * The decision, with the one distinction a screen must not lose.
 *
 * **`allow_for_project` does not mean the operation ran.** `request.rs:410`
 * says execution still goes through the approving human's own privilege, so an
 * allowed request keeps waiting for `apex request approve` at the machine. A
 * row drawing "allowed" as "done" would be telling the user work had happened
 * that had not — and the thing they would then not do is the thing that makes
 * it happen.
 */
internal fun decisionLine(r: PrivilegeRequest): String = when {
    r.isPending -> "waiting for a decision at the machine"
    r.isDenied -> "denied"
    r.awaitingExecution -> "allowed, but not run yet — it still has to be run at the machine"
    r.exitCode == 0 -> "allowed, and it ran"
    r.exitCode != null -> "allowed, and it failed with exit ${r.exitCode}"
    else -> "allowed"
}

@Composable
private fun GrantRow(project: String, keys: List<String>, onRevoke: (String?) -> Unit) {
    Column(Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 10.dp, bottom = 10.dp)) {
        Text(
            project,
            style = MachineText,
            maxLines = 1,
            overflow = TextOverflow.MiddleEllipsis,
        )
        Spacer(Modifier.height(4.dp))
        Text(
            if (keys.isEmpty()) {
                "no operations"
            } else {
                "agents here may run " + keys.joinToString(", ") + " without asking again"
            },
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Row {
            for (key in keys) {
                TextButton(onClick = { onRevoke(key) }) { Text("Revoke $key") }
            }
        }
        if (keys.size > 1) {
            TextButton(onClick = { onRevoke(null) }) { Text("Revoke all for this project") }
        }
    }
    HorizontalDivider(color = MaterialTheme.colorScheme.surfaceVariant)
}

@Composable
private fun SystemGrantRow(grant: SystemGrant, state: GrantState, onRevoke: () -> Unit) {
    Column(Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 10.dp, bottom = 10.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(
                if (grant.isBreakGlass) "Break glass" else "System access",
                style = MaterialTheme.typography.titleSmall,
                color = if (grant.isBreakGlass) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurface,
            )
            Spacer(Modifier.weight(1f))
            // The daemon's own state word and sentence. Sent rather than
            // derived, because a grant's state depends on the running kernel's
            // boot id — which a phone cannot read for a machine it is not on.
            Text(state.sentence, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        Text(
            listOfNotNull(grant.agent.takeIf { it.isNotBlank() }, grant.project).joinToString(" · "),
            style = MachineText,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            maxLines = 1,
            overflow = TextOverflow.MiddleEllipsis,
        )
        Text(
            if (grant.capabilities.isEmpty()) {
                // An empty list is legal and means break-glass — everything.
                // Drawn as "no operations" this would be exactly backwards.
                "every root operation"
            } else {
                grant.capabilities.joinToString(", ")
            },
            style = MaterialTheme.typography.bodySmall,
        )
        grant.authenticatedBy.takeIf { it.isNotBlank() }?.let {
            Text(
                "authorised by $it",
                style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        TextButton(onClick = onRevoke) { Text("End it now") }
    }
    HorizontalDivider(color = MaterialTheme.colorScheme.surfaceVariant)
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

@Composable
private fun Nothing() {
    Column(
        Modifier.fillMaxWidth().padding(32.dp),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text("Nothing is waiting", style = MaterialTheme.typography.titleMedium)
        Spacer(Modifier.height(8.dp))
        Text(
            "No agent has asked for a privileged operation, and nothing has standing " +
                "permission on this machine.",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}
