package com.apexos.remote.ui.agent

import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.apexos.remote.core.agent.Help
import com.apexos.remote.ui.theme.labelColumnWidth

/**
 * The in-app guide (P1-060's last criterion).
 *
 * The words are in `:core`, where `HelpParityTest` can hold them against the
 * desktop's own `AgentHelpContent.qml` and `tools/check-help-prose.sh` can run
 * the stop-slop checker over them. This file is only the rendering, which is
 * why there is no prose in it: a sentence written here would be a sentence
 * neither of those two ever looks at.
 *
 * ## Accessibility, and what is and is not claimed
 *
 * Everything here is text, laid out in a single scrolling column, in
 * MaterialTheme styles that follow the phone's font scale. There is no
 * icon-only control to label, no fixed-height box for text to be clipped out
 * of, and no `sp` size hardcoded past the theme. Those are the properties that
 * make a screen survive large text and a screen reader, and they are decided
 * by what is written here.
 *
 * Whether TalkBack actually reads it in a sensible order is NOT verified, and
 * cannot be from this repository: there is no device and no emulator, and
 * `assembleDebug` compiles code it never runs. The design is chosen to be the
 * one with the fewest ways to get that wrong, which is not the same as having
 * checked.
 *
 * `cmd` blocks scroll sideways in their own container rather than wrapping. A
 * wrapped command is a command somebody will mistype; a command that is cut
 * off is one they know is cut off.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun HelpScreen(onBack: () -> Unit) {
    Scaffold(
        containerColor = MaterialTheme.colorScheme.background,
        topBar = {
            TopAppBar(
                title = { Text(Help.ENTRY_LABEL, style = MaterialTheme.typography.titleMedium) },
                navigationIcon = { TextButton(onClick = onBack) { Text("Back") } },
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
            for (section in Help.sections) {
                Spacer(Modifier.height(20.dp))
                Text(
                    section.title,
                    style = MaterialTheme.typography.titleLarge,
                    modifier = Modifier.padding(horizontal = 16.dp),
                )
                Spacer(Modifier.height(4.dp))
                for (block in section.blocks) {
                    Spacer(Modifier.height(10.dp))
                    when (block.kind) {
                        Help.Kind.H -> Text(
                            block.text,
                            style = MaterialTheme.typography.titleSmall,
                            fontWeight = FontWeight.SemiBold,
                            modifier = Modifier.padding(horizontal = 16.dp),
                        )

                        Help.Kind.P -> Text(
                            block.text,
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            modifier = Modifier.padding(horizontal = 16.dp),
                        )

                        Help.Kind.CMD -> Box(
                            Modifier
                                .padding(horizontal = 16.dp)
                                .fillMaxWidth()
                                .clip(RoundedCornerShape(6.dp))
                                .background(MaterialTheme.colorScheme.surfaceVariant)
                                .horizontalScroll(rememberScrollState())
                                .padding(12.dp),
                        ) {
                            Text(
                                block.text,
                                style = MaterialTheme.typography.bodySmall,
                                fontFamily = FontFamily.Monospace,
                            )
                        }

                        Help.Kind.KV -> Row(Modifier.padding(horizontal = 16.dp)) {
                            Text(
                                block.term,
                                style = MaterialTheme.typography.bodyMedium,
                                fontWeight = FontWeight.SemiBold,
                                modifier = Modifier.width(labelColumnWidth()),
                            )
                            Spacer(Modifier.width(10.dp))
                            Text(
                                block.text,
                                style = MaterialTheme.typography.bodyMedium,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }

                        Help.Kind.NOTE -> Row(Modifier.padding(horizontal = 16.dp)) {
                            Box(
                                Modifier
                                    .width(3.dp)
                                    .height(IntrinsicNoteRule)
                                    .clip(RoundedCornerShape(2.dp))
                                    .background(MaterialTheme.colorScheme.primary),
                            )
                            Spacer(Modifier.width(10.dp))
                            Text(
                                block.text,
                                style = MaterialTheme.typography.bodyMedium,
                            )
                        }

                        // Rendered in the same weight as everything else and
                        // labelled in words, not by colour alone. A user who
                        // cannot distinguish the accent still has to be able to
                        // tell "APEX does not do this yet" from "here is how".
                        Help.Kind.TODO -> Text(
                            "Not in this build: ${block.text}",
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            modifier = Modifier.padding(horizontal = 16.dp),
                        )
                    }
                }
            }
            Spacer(Modifier.height(32.dp))
        }
    }
}

/**
 * How tall a note's accent rule is.
 *
 * A fixed height rather than one matched to the paragraph beside it, because
 * matching would need the text's measured height and a rule that grew with the
 * phone's font scale would push the layout around for no gain. It is decoration
 * beside text that is complete without it.
 */
private val IntrinsicNoteRule = 56.dp
