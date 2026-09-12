package com.apexos.remote.ui.term

import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectDragGesturesAfterLongPress
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TextField
import androidx.compose.material3.TextFieldDefaults
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.key.Key as ComposeKey
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.isAltPressed
import androidx.compose.ui.input.key.isCtrlPressed
import androidx.compose.ui.input.key.isShiftPressed
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.input.key.utf16CodePoint
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.unit.dp
import com.apexos.remote.core.Settings
import com.apexos.remote.core.term.AccessoryAction
import com.apexos.remote.core.term.AccessoryKey
import com.apexos.remote.core.term.AccessoryKeys
import com.apexos.remote.core.term.Key
import com.apexos.remote.core.term.Keys
import com.apexos.remote.core.term.Mods
import com.apexos.remote.core.term.Pos
import com.apexos.remote.core.term.Snapshot
import com.apexos.remote.ui.theme.ApexPalette

/**
 * A terminal, on a phone.
 *
 * Everything in here that is a rule rather than a layout lives in `:core`:
 * scrolling, searching, selecting and what a selection copies are `Viewport`'s;
 * what a key sends is `Keys`'; what a cell looks like is `Palette`'s. This
 * file is the part that cannot be tested on a machine with no phone, and it is
 * deliberately only that part.
 */
@OptIn(ExperimentalMaterial3Api::class)
// `LocalClipboardManager` is deprecated in favour of `LocalClipboard`, whose
// API is suspend-only and whose entries are built from Android `ClipData`.
// Copying one line of terminal text gains nothing from either, and the
// replacement would put a coroutine between a button and the clipboard.
@Suppress("DEPRECATION")
@Composable
fun TerminalScreen(
    controller: TerminalController,
    title: String,
    settings: Settings,
    onBack: () -> Unit,
) {
    val clipboard = LocalClipboardManager.current
    val keyboard = LocalSoftwareKeyboardController.current
    val focus = remember { FocusRequester() }
    val viewport = controller.viewport
    val terminal = controller.terminal

    val (measurer, metrics) = rememberCellMetrics(settings.terminalTextSp)

    var snapshot by remember { mutableStateOf<Snapshot?>(null) }
    var generation by remember { mutableStateOf(0L) }
    var searching by remember { mutableStateOf(false) }
    var query by remember { mutableStateOf("") }
    var armed by remember { mutableStateOf(Mods.NONE) }
    var grid by remember { mutableStateOf(80 to 24) }

    // The repaint loop. It runs once per displayed frame and does nothing at
    // all unless something changed — which is what stops a fast `cat` from
    // spending the whole frame budget in layout. `generation` is bumped by
    // everything that changes the view without changing the terminal:
    // scrolling, searching, selecting.
    LaunchedEffect(controller, metrics) {
        var lastRevision = -1L
        var lastGeneration = -1L
        while (true) {
            withFrameNanos { }
            val revision = terminal.revision
            if (revision != lastRevision || generation != lastGeneration) {
                lastRevision = revision
                lastGeneration = generation
                snapshot = viewport.snapshot()
            }
        }
    }

    DisposableEffect(controller) {
        controller.start()
        onDispose { }
    }

    Scaffold(
        containerColor = MaterialTheme.colorScheme.background,
        topBar = {
            TopAppBar(
                title = { Text(title, style = MaterialTheme.typography.titleMedium, maxLines = 1) },
                navigationIcon = { TextButton(onClick = onBack) { Text("Back") } },
                actions = {
                    TextButton(onClick = {
                        searching = !searching
                        if (!searching) {
                            viewport.clearSearch()
                            generation++
                        }
                    }) { Text(if (searching) "Close" else "Find") }
                },
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
                .imePadding(),
        ) {
            StatusStrip(controller)

            if (searching) {
                SearchBar(
                    query = query,
                    matches = viewport.matchCount,
                    current = viewport.current,
                    onQuery = {
                        query = it
                        viewport.find(it)
                        generation++
                    },
                    onNext = {
                        viewport.findNext()
                        generation++
                    },
                    onPrevious = {
                        viewport.findPrevious()
                        generation++
                    },
                )
            }

            Box(
                Modifier
                    .weight(1f)
                    .fillMaxWidth()
                    .background(Color(0xFF1A282A))
                    .onSizeChanged { size ->
                        // The orientation criterion, and it is this and not a
                        // configuration listener: a rotation, a keyboard
                        // appearing and a split-screen resize all arrive here,
                        // and all three need the same answer.
                        val cols = metrics.cols(size.width.toFloat())
                        val rows = metrics.rows(size.height.toFloat())
                        if (cols to rows != grid) {
                            grid = cols to rows
                            controller.resize(cols, rows)
                            generation++
                        }
                    }
                    .pointerInputForSelection(
                        metrics = metrics,
                        snapshot = snapshot,
                        onTap = {
                            viewport.clearSelection()
                            generation++
                            focus.requestFocus()
                            keyboard?.show()
                        },
                        onWord = { pos ->
                            viewport.selectWord(pos)
                            generation++
                        },
                        onDragStart = { pos ->
                            viewport.selectFrom(pos)
                            generation++
                        },
                        onDrag = { pos ->
                            viewport.selectTo(pos)
                            generation++
                        },
                    )
                    .pointerInputForScroll(metrics) { lines ->
                        viewport.scrollBy(lines)
                        generation++
                    },
            ) {
                snapshot?.let {
                    TerminalView(
                        snapshot = it,
                        measurer = measurer,
                        metrics = metrics,
                        textSp = settings.terminalTextSp,
                        selectionColour = ApexPalette.Teal.copy(alpha = 0.30f),
                        matchColour = ApexPalette.Teal.copy(alpha = 0.18f),
                        currentMatchColour = ApexPalette.Teal.copy(alpha = 0.55f),
                        cursorColour = ApexPalette.Teal,
                        modifier = Modifier.fillMaxSize(),
                    )
                }

                // The hidden field that makes the software keyboard appear and
                // delivers what it types. Zero-sized and always empty: an IME
                // with text in it would try to edit that text, and a terminal
                // is not an editable document — every character is an event
                // that has already been sent.
                BasicTextField(
                    value = TextFieldValue(""),
                    onValueChange = { value ->
                        if (value.text.isNotEmpty()) {
                            controller.sendText(value.text.replace("\n", "\r"), armed)
                            armed = Mods.NONE
                            generation++
                        }
                    },
                    modifier = Modifier
                        .size(1.dp)
                        .focusRequester(focus)
                        .onPreviewKeyEvent { event -> handleKey(event, controller, armed) { armed = Mods.NONE } },
                    // No autocorrect and no capitalisation: a terminal is not
                    // prose, and an IME that helpfully capitalised `git` would
                    // be sending `Git`.
                    keyboardOptions = KeyboardOptions(
                        capitalization = KeyboardCapitalization.None,
                        autoCorrectEnabled = false,
                        imeAction = ImeAction.None,
                    ),
                    textStyle = TextStyle(color = Color.Transparent),
                    cursorBrush = androidx.compose.ui.graphics.SolidColor(Color.Transparent),
                )

                if (snapshot?.following == false) {
                    JumpToLatest(
                        Modifier.align(Alignment.BottomEnd),
                        onClick = {
                            viewport.toBottom()
                            generation++
                        },
                    )
                }
            }

            if (viewport.hasSelection) {
                SelectionBar(
                    onCopy = {
                        viewport.selectedText()?.let { clipboard.setText(AnnotatedString(it)) }
                        viewport.clearSelection()
                        generation++
                    },
                    onPaste = {
                        clipboard.getText()?.text?.let { controller.paste(it) }
                        viewport.clearSelection()
                        generation++
                    },
                    onCancel = {
                        viewport.clearSelection()
                        generation++
                    },
                )
            }

            AccessoryRow(
                keys = settings.accessoryRow,
                armed = armed,
                onAction = { action ->
                    when (action) {
                        is AccessoryAction.Modifier -> {
                            // A modifier ARMS rather than sends, the way a
                            // phone's shift key works: there is no chord on a
                            // touchscreen, so Ctrl must survive until the next
                            // press.
                            armed = Mods(
                                ctrl = if (action.ctrl) !armed.ctrl else armed.ctrl,
                                alt = if (action.alt) !armed.alt else armed.alt,
                            )
                        }
                        else -> {
                            Keys.accessory(action, terminal.applicationCursorKeys, armed)
                                ?.let { controller.send(it) }
                            armed = Mods.NONE
                        }
                    }
                    generation++
                },
                onPaste = {
                    clipboard.getText()?.text?.let { controller.paste(it) }
                    generation++
                },
            )
        }
    }

    LaunchedEffect(Unit) { focus.requestFocus() }
}

/**
 * A hardware key, or the one software key that is not text.
 *
 * Backspace is the case worth naming. The field this listens on is kept empty,
 * so there is never anything for a delete to remove — which means an IME's
 * backspace arrives here as a key event and nowhere else. A build that handled
 * only `onValueChange` would have a terminal you cannot correct a typo in.
 */
private fun handleKey(
    event: androidx.compose.ui.input.key.KeyEvent,
    controller: TerminalController,
    armed: Mods,
    clearArmed: () -> Unit,
): Boolean {
    if (event.type != KeyEventType.KeyDown) return false
    val mods = Mods(
        ctrl = event.isCtrlPressed || armed.ctrl,
        alt = event.isAltPressed || armed.alt,
        shift = event.isShiftPressed,
    )
    val named = when (event.key) {
        ComposeKey.Backspace -> Key.BACKSPACE
        ComposeKey.Enter, ComposeKey.NumPadEnter -> Key.ENTER
        ComposeKey.Tab -> Key.TAB
        ComposeKey.Escape -> Key.ESCAPE
        ComposeKey.DirectionUp -> Key.UP
        ComposeKey.DirectionDown -> Key.DOWN
        ComposeKey.DirectionLeft -> Key.LEFT
        ComposeKey.DirectionRight -> Key.RIGHT
        ComposeKey.MoveHome -> Key.HOME
        ComposeKey.MoveEnd -> Key.END
        ComposeKey.PageUp -> Key.PAGE_UP
        ComposeKey.PageDown -> Key.PAGE_DOWN
        ComposeKey.Insert -> Key.INSERT
        ComposeKey.Delete -> Key.DELETE
        ComposeKey.F1 -> Key.F1
        ComposeKey.F2 -> Key.F2
        ComposeKey.F3 -> Key.F3
        ComposeKey.F4 -> Key.F4
        ComposeKey.F5 -> Key.F5
        ComposeKey.F6 -> Key.F6
        ComposeKey.F7 -> Key.F7
        ComposeKey.F8 -> Key.F8
        ComposeKey.F9 -> Key.F9
        ComposeKey.F10 -> Key.F10
        ComposeKey.F11 -> Key.F11
        ComposeKey.F12 -> Key.F12
        else -> null
    }
    if (named != null) {
        controller.send(Keys.encode(named, mods, controller.terminal.applicationCursorKeys))
        clearArmed()
        return true
    }
    // A printable character with a modifier held. Without one it is left to
    // the IME, which is what delivers ordinary typing.
    val code = event.utf16CodePoint
    if (code != 0 && (mods.ctrl || mods.alt) && code in 0x20..0x10FFFF) {
        controller.sendText(String(Character.toChars(code)), mods)
        clearArmed()
        return true
    }
    return false
}

@Composable
private fun StatusStrip(controller: TerminalController) {
    val status = controller.status
    val (text, alarming) = when (status) {
        is TerminalStatus.Connecting ->
            (if (status.attempt == 1) "Connecting…" else "Reconnecting, attempt ${status.attempt}…") to false
        is TerminalStatus.Live ->
            if (controller.attachments > 1) "Re-attached to ${status.machine}" to false else null to false
        is TerminalStatus.Reconnecting ->
            ("Connection lost" + (status.why?.let { " — $it" } ?: "") + ". Reconnecting…") to true
        is TerminalStatus.Ended -> ("The session ended" + status.reason.ifEmpty { "" }) to true
        is TerminalStatus.Refused -> status.reply to true
        TerminalStatus.Detached -> "Detached. The session is still running." to false
    }
    val dropped = controller.droppedInput
    if (text == null && !dropped) return
    Surface(
        color = if (alarming || dropped) {
            MaterialTheme.colorScheme.errorContainer
        } else {
            MaterialTheme.colorScheme.surfaceVariant
        },
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column(Modifier.padding(horizontal = 14.dp, vertical = 8.dp)) {
            text?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
            if (dropped) {
                // Said out loud, because the alternative is somebody believing
                // they answered a prompt. `PtyAttachment` drops input typed
                // while disconnected on purpose — a `y` delivered thirty
                // seconds later goes into whatever the agent is doing now.
                Text(
                    "What you typed while disconnected was not sent.",
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        }
    }
}

@Composable
private fun SearchBar(
    query: String,
    matches: Int,
    current: Int,
    onQuery: (String) -> Unit,
    onNext: () -> Unit,
    onPrevious: () -> Unit,
) {
    Row(
        Modifier
            .fillMaxWidth()
            .background(MaterialTheme.colorScheme.surfaceVariant)
            .padding(horizontal = 8.dp, vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        TextField(
            value = query,
            onValueChange = onQuery,
            singleLine = true,
            placeholder = { Text("Find in scrollback") },
            modifier = Modifier.weight(1f),
            colors = TextFieldDefaults.colors(
                focusedContainerColor = Color.Transparent,
                unfocusedContainerColor = Color.Transparent,
            ),
        )
        Text(
            // `0 of 0` rather than nothing, so an unsuccessful search is
            // visibly a search that found nothing rather than one that did not
            // run.
            if (matches == 0) "0" else "${current + 1}/$matches",
            style = MaterialTheme.typography.labelSmall,
            modifier = Modifier.padding(horizontal = 8.dp),
        )
        TextButton(onClick = onPrevious, enabled = matches > 0) { Text("↑") }
        TextButton(onClick = onNext, enabled = matches > 0) { Text("↓") }
    }
}

@Composable
private fun SelectionBar(onCopy: () -> Unit, onPaste: () -> Unit, onCancel: () -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .background(MaterialTheme.colorScheme.surfaceVariant)
            .padding(horizontal = 8.dp, vertical = 2.dp),
        horizontalArrangement = Arrangement.spacedBy(4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        TextButton(onClick = onCopy) { Text("Copy") }
        TextButton(onClick = onPaste) { Text("Paste") }
        Spacer(Modifier.weight(1f))
        TextButton(onClick = onCancel) { Text("Cancel") }
    }
}

@Composable
private fun AccessoryRow(
    keys: List<AccessoryKey>,
    armed: Mods,
    onAction: (AccessoryAction) -> Unit,
    onPaste: () -> Unit,
) {
    Surface(color = MaterialTheme.colorScheme.surfaceVariant, modifier = Modifier.fillMaxWidth()) {
        LazyRow(
            Modifier.padding(vertical = 4.dp),
            horizontalArrangement = Arrangement.spacedBy(4.dp),
            contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 6.dp),
        ) {
            items(keys, key = { it.label + it.action }) { key ->
                // A stored action this build does not understand is not shown
                // rather than crashing the row: `AccessoryKeys.parse` answers
                // null, and a row written by a newer version must not stop the
                // app opening.
                val action = AccessoryKeys.parse(key.action) ?: return@items
                val lit = action is AccessoryAction.Modifier &&
                    ((action.ctrl && armed.ctrl) || (action.alt && armed.alt))
                AccessoryButton(key.label, lit) { onAction(action) }
            }
            item { AccessoryButton("Paste", false, onPaste) }
        }
    }
}

@Composable
private fun AccessoryButton(label: String, lit: Boolean, onClick: () -> Unit) {
    Surface(
        color = if (lit) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.surface,
        contentColor = if (lit) MaterialTheme.colorScheme.onPrimary else MaterialTheme.colorScheme.onSurface,
        shape = RoundedCornerShape(6.dp),
        modifier = Modifier
            .height(38.dp)
            .clip(RoundedCornerShape(6.dp)),
    ) {
        TextButton(onClick = onClick, modifier = Modifier.height(38.dp)) {
            Text(label, style = MaterialTheme.typography.labelSmall)
        }
    }
}

@Composable
private fun JumpToLatest(modifier: Modifier, onClick: () -> Unit) {
    Surface(
        color = MaterialTheme.colorScheme.primary,
        contentColor = MaterialTheme.colorScheme.onPrimary,
        shape = RoundedCornerShape(18.dp),
        modifier = modifier.padding(16.dp),
    ) {
        TextButton(onClick = onClick) { Text("Latest ↓") }
    }
}

/** Taps, double-taps and a long-press drag, all converted to grid positions. */
private fun Modifier.pointerInputForSelection(
    metrics: CellMetrics,
    snapshot: Snapshot?,
    onTap: () -> Unit,
    onWord: (Pos) -> Unit,
    onDragStart: (Pos) -> Unit,
    onDrag: (Pos) -> Unit,
): Modifier = this
    .pointerInput(metrics, snapshot) {
        detectTapGestures(
            onTap = { onTap() },
            onDoubleTap = { offset -> posAt(snapshot, metrics, offset)?.let(onWord) },
        )
    }
    .pointerInput(metrics, snapshot) {
        // A long press starts a selection and the same gesture extends it,
        // which is how every other Android text surface behaves. Two separate
        // pointerInput blocks because a tap detector and a drag detector each
        // consume the whole gesture stream.
        detectDragGesturesAfterLongPress(
            onDragStart = { offset -> posAt(snapshot, metrics, offset)?.let(onDragStart) },
            onDrag = { change, _ -> posAt(snapshot, metrics, change.position)?.let(onDrag) },
        )
    }

/** A touch point turned into a grid position, or null when nothing is drawn yet. */
private fun posAt(snapshot: Snapshot?, metrics: CellMetrics, offset: Offset): Pos? {
    val snap = snapshot ?: return null
    val row = (offset.y / metrics.height).toInt()
    val col = (offset.x / metrics.width).toInt()
    return snap.posAt(row, col)
}

/** A vertical drag scrolls the history, in whole lines. */
private fun Modifier.pointerInputForScroll(
    metrics: CellMetrics,
    onScroll: (Int) -> Unit,
): Modifier = this.pointerInput(metrics) {
    var carried = 0f
    detectVerticalDragGestures(
        onDragEnd = { carried = 0f },
    ) { _, delta ->
        // Carried rather than rounded per event, so a slow drag still moves:
        // a gesture that produced half a line per frame and rounded to zero
        // would feel like a terminal that cannot be scrolled at all.
        carried += delta
        val lines = (carried / metrics.height).toInt()
        if (lines != 0) {
            carried -= lines * metrics.height
            onScroll(-lines)
        }
    }
}
