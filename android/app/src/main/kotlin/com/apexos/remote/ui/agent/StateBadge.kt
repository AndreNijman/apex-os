package com.apexos.remote.ui.agent

import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.border
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.apexos.remote.core.agent.AgentStates
import com.apexos.remote.core.agent.StateGlyphs
import com.apexos.remote.core.agent.Weight
import com.apexos.remote.ui.theme.ApexTones

/**
 * The one place an agent's state becomes something you can see, on a phone.
 *
 * A port of `apex-shell/src/services/agents/StateBadge.qml`, which derives all
 * of tone, weight and fill from `sessionState` alone — it takes no agent
 * parameter at all, and that is what makes every session kind render
 * identically. Same here.
 *
 * ## Why a weight and not just a hue
 *
 * Around one man in twelve cannot use hue to separate red from green, and the
 * two states that pairing fails on are `failed` and `done` — precisely the
 * pair where being wrong costs the most. So every tone carries a weight, and
 * the weight changes the badge's shape and its ink rather than its colour:
 *
 * * **solid** — a filled chip, glyph in whichever fixed ink reads on it.
 *   `blocked` and `failed`: the two that mean somebody has to do something.
 * * **tint** — the tone at 0.18, glyph in the tone. `working`.
 * * **outline** — a ring, nothing filled. `waiting`: present, not shouting.
 * * **plain** — no chip. `done` and `idle`; a finished session is not a status.
 *
 * With the glyph and the written label, the encoding is fourfold: shape,
 * colour, glyph, word. `agentstate.js` carries the measurements behind the
 * assignment and `AgentStateAgreementTest` checks this build agrees with it.
 */
@Composable
fun StateBadge(state: String?, size: Dp = 24.dp, modifier: Modifier = Modifier) {
    val tone = ApexTones.forState(state)
    val weight = AgentStates.weight(state)
    val shape = RoundedCornerShape(size * 0.3f)

    // Only a working session animates, and the animation is on the GLYPH and
    // not the chip. Motion in a status list should mean "this is changing", or
    // it is noise — and a pulsing fill next to four static ones reads as a
    // rendering fault rather than as progress.
    val pulse = if (state == AgentStates.WORKING) {
        val transition = rememberInfiniteTransition(label = "working")
        val value by transition.animateFloat(
            initialValue = 1f,
            targetValue = 0.45f,
            animationSpec = infiniteRepeatable(tween(900), RepeatMode.Reverse),
            label = "workingAlpha",
        )
        value
    } else {
        1f
    }

    Box(
        modifier
            .size(size)
            .clip(shape)
            .background(ApexTones.chipFill(weight, tone))
            .then(
                if (weight == Weight.OUTLINE) Modifier.border(1.dp, tone, shape) else Modifier,
            ),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            // The platform glyph rather than the shell's Nerd Font one: a
            // phone has no Nerd Font, and a missing glyph is a tofu box in the
            // one place a person looks to see what is happening.
            // `StateGlyphs.nerd` is carried for a build that bundles the font.
            text = StateGlyphs.ascii(state),
            color = ApexTones.chipInk(weight, tone),
            style = TextStyle(fontSize = (size.value * 0.52f).sp, textAlign = TextAlign.Center),
            modifier = Modifier.alpha(pulse),
        )
    }
}

/** The written half of the encoding: a state a person can read. */
@Composable
fun StateLabel(state: String?): String = AgentStates.label(state)
