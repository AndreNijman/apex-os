package com.apexos.remote.ui.term

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.ExperimentalTextApi
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.text.TextMeasurer
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.sp
import com.apexos.remote.core.term.Attrs
import com.apexos.remote.core.term.Cell
import com.apexos.remote.core.term.Palette
import com.apexos.remote.core.term.Snapshot
import com.apexos.remote.core.term.SnapshotRow
import com.apexos.remote.core.term.SpanKind
import com.apexos.remote.ui.theme.TerminalDefaults
import com.apexos.remote.ui.theme.terminalColour
import kotlin.math.floor
import kotlin.math.max

/**
 * How big a cell is, and therefore how big the grid is.
 *
 * Measured from the font rather than assumed, because the whole point of the
 * number is that eighty columns fit. `FontFamily.Monospace` on Android is
 * whatever the device ships, and its advance width is not the same on every
 * one of them — a hard-coded 0.6 em would put the eightieth column off the
 * edge on some phones and leave a margin on others.
 */
@Immutable
data class CellMetrics(val width: Float, val height: Float, val baseline: Float) {
    fun cols(widthPx: Float): Int = max(1, floor(widthPx / width).toInt())

    fun rows(heightPx: Float): Int = max(1, floor(heightPx / height).toInt())
}

@OptIn(ExperimentalTextApi::class)
@Composable
fun rememberCellMetrics(textSp: Float): Pair<TextMeasurer, CellMetrics> {
    val measurer = rememberTextMeasurer()
    val density = LocalDensity.current
    val metrics = remember(textSp, density, measurer) { measure(measurer, density, textSp.sp) }
    return measurer to metrics
}

@OptIn(ExperimentalTextApi::class)
private fun measure(measurer: TextMeasurer, density: Density, size: TextUnit): CellMetrics {
    // A capital M, which is the widest glyph in a proportional font and
    // exactly as wide as everything else in a monospaced one — so measuring it
    // is a check that the font really is monospaced as well as a measurement.
    val laid: TextLayoutResult = measurer.measure(
        text = "M",
        style = TextStyle(fontFamily = FontFamily.Monospace, fontSize = size),
        density = density,
    )
    val w = laid.size.width.toFloat()
    // Line height with a little air. A terminal packed to the exact glyph box
    // is legible on a laptop and a grey smear on a phone.
    val h = laid.size.height.toFloat() * LINE_SPACING
    return CellMetrics(width = if (w > 0) w else 1f, height = if (h > 0) h else 1f, baseline = laid.firstBaseline)
}

/** 1.15, which is what makes eight-point text readable held at arm's length. */
private const val LINE_SPACING = 1.15f

/**
 * The grid, drawn.
 *
 * ## Runs, not cells
 *
 * An 80x40 grid is 3200 cells, and a `drawText` per cell is 3200 text layouts
 * per frame — which on a mid-range phone is somewhere between a dropped frame
 * and a visibly stuttering terminal. Terminal output is overwhelmingly long
 * runs of one style, so each row is walked once and split into runs of equal
 * ink, and each run is one background rectangle and one text draw.
 *
 * ## What is not drawn
 *
 * A wide character's tail cell, which holds [Cell.WIDE_TAIL] rather than a
 * space precisely so it can be skipped: drawing a space there would put a gap
 * through the middle of a CJK glyph.
 */
@OptIn(ExperimentalTextApi::class)
@Composable
fun TerminalView(
    snapshot: Snapshot,
    measurer: TextMeasurer,
    metrics: CellMetrics,
    textSp: Float,
    selectionColour: Color,
    matchColour: Color,
    currentMatchColour: Color,
    cursorColour: Color,
    modifier: Modifier = Modifier,
) {
    val baseStyle = remember(textSp) {
        TextStyle(fontFamily = FontFamily.Monospace, fontSize = textSp.sp)
    }
    Box(modifier) {
        Canvas(Modifier.fillMaxSize()) {
            drawRect(terminalColour(TerminalDefaults.Background))
            for ((row, line) in snapshot.rows.withIndex()) {
                drawRow(
                    line = line,
                    row = row,
                    metrics = metrics,
                    measurer = measurer,
                    baseStyle = baseStyle,
                    selectionColour = selectionColour,
                    matchColour = matchColour,
                    currentMatchColour = currentMatchColour,
                )
            }
            if (snapshot.cursorVisible && snapshot.cursorRow >= 0) {
                // A block, at 45% so the character under it stays readable.
                // A terminal cursor that hid its own character is one people
                // lose their place in.
                drawRect(
                    color = cursorColour.copy(alpha = 0.45f),
                    topLeft = Offset(snapshot.cursorCol * metrics.width, snapshot.cursorRow * metrics.height),
                    size = Size(metrics.width, metrics.height),
                )
            }
        }
    }
}

@OptIn(ExperimentalTextApi::class)
private fun DrawScope.drawRow(
    line: SnapshotRow,
    row: Int,
    metrics: CellMetrics,
    measurer: TextMeasurer,
    baseStyle: TextStyle,
    selectionColour: Color,
    matchColour: Color,
    currentMatchColour: Color,
) {
    val y = row * metrics.height
    var col = 0
    while (col < line.cols) {
        val cell = line.cellAt(col)
        if (cell.code == Cell.WIDE_TAIL) {
            col++
            continue
        }
        val ink = Palette.render(cell, TerminalDefaults.Foreground, TerminalDefaults.Background)
        val attrs = cell.attrs
        val span = line.spanAt(col)
        // Gather every following cell that would be drawn identically.
        var end = col + 1
        while (end < line.cols) {
            val next = line.cellAt(end)
            if (next.code == Cell.WIDE_TAIL) {
                end++
                continue
            }
            if (next.attrs != attrs) break
            if (line.spanAt(end) != span) break
            if (Palette.render(next, TerminalDefaults.Foreground, TerminalDefaults.Background) != ink) break
            end++
        }
        val x = col * metrics.width
        val width = (end - col) * metrics.width

        val background = when (span) {
            SpanKind.SELECTION -> selectionColour
            SpanKind.MATCH -> matchColour
            SpanKind.CURRENT_MATCH -> currentMatchColour
            null -> terminalColour(ink.bg)
        }
        if (span != null || ink.bg != TerminalDefaults.Background) {
            drawRect(background, topLeft = Offset(x, y), size = Size(width, metrics.height))
        }

        val text = buildString {
            for (c in col until end) {
                val code = line.code[c]
                if (code != Cell.WIDE_TAIL) appendCodePoint(code)
            }
        }
        if (text.isNotBlank()) {
            val foreground = terminalColour(ink.fg)
            drawText(
                textMeasurer = measurer,
                text = text,
                topLeft = Offset(x, y),
                style = baseStyle.copy(
                    color = foreground,
                    // Bold is a weight AND a brightening; `Palette.render` did
                    // the second half, this does the first.
                    fontWeight = if (Attrs.has(attrs, Attrs.BOLD)) FontWeight.Bold else FontWeight.Normal,
                    fontStyle = if (Attrs.has(attrs, Attrs.ITALIC)) FontStyle.Italic else FontStyle.Normal,
                ),
            )
            // Underline and strike as lines rather than as a TextDecoration:
            // a decoration is part of the layout and forces a re-measure per
            // run, and these are two rectangles.
            if (Attrs.has(attrs, Attrs.UNDERLINE)) {
                drawRect(
                    foreground,
                    topLeft = Offset(x, y + metrics.height - RULE),
                    size = Size(width, RULE),
                )
            }
            if (Attrs.has(attrs, Attrs.STRIKE)) {
                drawRect(
                    foreground,
                    topLeft = Offset(x, y + metrics.height / 2f),
                    size = Size(width, RULE),
                )
            }
        }
        col = end
    }
}

private const val RULE = 1.5f
