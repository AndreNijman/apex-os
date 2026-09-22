package com.apexos.remote.ui.term

import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.SemanticsPropertyReceiver
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.semantics.text
import androidx.compose.ui.text.AnnotatedString
import com.apexos.remote.core.term.TerminalReading

/**
 * What the terminal puts into the semantics tree.
 *
 * ## Why this is a function on the receiver and not a `Modifier`
 *
 * So that it can be tested without a phone. `Modifier.semantics { }` cannot be
 * evaluated outside a composition, but the lambda it takes is an ordinary
 * function on [SemanticsPropertyReceiver] — and a test can implement that
 * interface in ten lines and read back every property that was set. That is
 * the difference between asserting that a label exists and asserting that a
 * file mentions the word `contentDescription`.
 *
 * ## What a screen reader gets, and why each piece is the piece it is
 *
 * `TerminalView` is a `Canvas`: the grid is painted with `drawText` and not
 * one glyph of it is a composable, so **nothing on this screen reaches the
 * semantics tree unless it is put there**. Four things are, and a fifth
 * deliberately is not.
 *
 * * **`text`, not `contentDescription`.** TalkBack moves through a node's text
 *   at whatever granularity the user has chosen — character, word, line — and
 *   line-by-line movement across the grid is what reading a terminal *is* for
 *   somebody who cannot see it. A `contentDescription` is a single utterance
 *   that can only be replayed from the start, which turns a forty-line build
 *   log into one forty-line sentence.
 * * **`stateDescription`** carries the position: which lines are showing, out
 *   of how many, where the cursor is, and whether the view is live or held in
 *   the history. Those are the four facts a sighted user reads off the scroll
 *   position and the cursor without noticing they are doing it.
 * * **`onClick` with a label.** A tap on this screen raises the keyboard, and
 *   until now it was a bare `pointerInput` — invisible to the semantics tree,
 *   so a reader could not activate it at all and announced nothing if it
 *   tried. With the action set, TalkBack says "double tap to type".
 * * **Custom actions** for the two things that are otherwise a target on a
 *   grid nobody can see: returning to the live output, and copying what is on
 *   screen. Both exist as buttons; neither is reachable by somebody who cannot
 *   find the button.
 * * **No live region here.** See [terminalAnnouncement] — a live region on a
 *   node whose text is the whole screen re-reads the whole screen on every
 *   byte, and the user hears the first two words of each of forty
 *   interruptions.
 */
fun SemanticsPropertyReceiver.terminalSemantics(
    reading: TerminalReading,
    /** What a double-tap does. Null when the terminal cannot take input. */
    onActivate: (() -> Boolean)? = null,
    actions: List<CustomAccessibilityAction> = emptyList(),
) {
    text = AnnotatedString(reading.text)
    stateDescription = reading.state
    if (onActivate != null) onClick(label = ACTIVATE_LABEL, action = onActivate)
    if (actions.isNotEmpty()) customActions = actions
}

/**
 * The label on the terminal's own tap.
 *
 * TalkBack reads it as "double tap to <label>", so it is a verb phrase and not
 * a description of the control.
 */
const val ACTIVATE_LABEL: String = "type"

/**
 * The node that says what just arrived.
 *
 * ## Why it is a second node
 *
 * A live region is announced when its description *changes*, and the thing
 * that has to change is short: the lines that just finished. Putting the
 * region on the grid node would make every arriving byte an announcement of
 * the entire visible buffer — forty lines, discarded two words in by the next
 * one. `TerminalAnnouncer` in `:core` decides what this says; this is only
 * where it is said.
 *
 * ## Why `contentDescription` here, where the grid uses `text`
 *
 * The opposite reason to the grid's. This is not something to explore, it is
 * something to be told once: an utterance, which is exactly what a
 * `contentDescription` is. It is also what a live region reads — Android
 * re-announces the description, and a node whose text changed but whose
 * description did not would be silent.
 */
fun SemanticsPropertyReceiver.terminalAnnouncement(announcement: String) {
    contentDescription = announcement
    liveRegion = LiveRegionMode.Polite
}
