package io.github.code_akram.or2.terminal

import androidx.compose.ui.graphics.toArgb
import io.github.code_akram.or2.ffi.CellLink
import io.github.code_akram.or2.ffi.CellStyle
import io.github.code_akram.or2.ffi.CellWidth
import io.github.code_akram.or2.ffi.Scrollback
import io.github.code_akram.or2.ffi.TerminalCursor
import io.github.code_akram.or2.ffi.TerminalFrame
import io.github.code_akram.or2.ui.Or2Colors
import kotlin.math.floor

/**
 * The terminal's default background until the first frame: the theme's terminal colour, which is
 * the core's own default (core/terminal.rs; a test compares it with a real frame).
 */
val DefaultBackground: UInt = (Or2Colors.TerminalBackground.toArgb() and 0xFFFFFF).toUInt()

data class ResolvedCell(val text: String, val width: CellWidth, val style: CellStyle)
/** [links] are the row's OSC 8 hyperlinks (inclusive column runs), empty when none. */
data class ResolvedRow(val cells: List<ResolvedCell>, val wrapped: Boolean, val links: List<CellLink> = emptyList())
data class GridSize(val columns: UShort, val rows: UShort)

fun gridSize(width: Int, height: Int, cellWidth: Float, cellHeight: Float): GridSize? {
    if (width <= 0 || height <= 0 || cellWidth <= 0 || cellHeight <= 0) return null
    val columns = floor(width / cellWidth).toInt()
    val rows = floor(height / cellHeight).toInt()
    if (columns <= 0 || rows <= 0) return null
    return GridSize(columns.coerceAtMost(65535).toUShort(), rows.coerceAtMost(65535).toUShort())
}

/** Styles are resolved at ingress, never indexed through a subsequent frame's table. */
class TerminalGrid {
    var columns = 0
        private set
    var rows: List<ResolvedRow> = emptyList()
        private set
    var cursor: TerminalCursor? = null
        private set
    var background = DefaultBackground
        private set
    var scrollback = Scrollback(0u, 0u)
        private set
    var sequence = 0uL
        private set
    val hasGrid get() = rows.isNotEmpty()

    /** False means a new view received a delta before its requested full snapshot. */
    fun apply(frame: TerminalFrame): Boolean {
        if (!frame.full && (!hasGrid || columns != frame.columns.toInt() || rows.size != frame.rows.toInt())) {
            return false
        }
        val changed = frame.changedRows.associate { row ->
            row.index.toInt() to ResolvedRow(row.cells.map { cell ->
                ResolvedCell(cell.text, cell.width, frame.styles[cell.style.toInt()])
            }, row.wrapped, row.links)
        }
        rows = if (frame.full) {
            List(frame.rows.toInt()) { changed.getValue(it) }
        } else {
            rows.mapIndexed { index, row -> changed[index] ?: row }
        }
        columns = frame.columns.toInt()
        cursor = frame.cursor
        background = frame.background
        scrollback = frame.scrollback
        sequence = frame.sequence
        return true
    }
}

/** Bounded, content-free timings in milliseconds, readable by debug UI and device tests. */
class FrameTimings(private val capacity: Int = 240) {
    private val samples = DoubleArray(capacity)
    private var next = 0
    var count = 0
        private set

    fun record(nanos: Long) {
        samples[next] = nanos / 1_000_000.0
        next = (next + 1) % capacity
        count = (count + 1).coerceAtMost(capacity)
    }

    fun clear() {
        next = 0
        count = 0
    }

    fun percentile(percent: Int): Double {
        if (count == 0) return 0.0
        val sorted = samples.take(count).sorted()
        return sorted[(kotlin.math.ceil(percent / 100.0 * count).toInt() - 1).coerceIn(0, count - 1)]
    }
}
