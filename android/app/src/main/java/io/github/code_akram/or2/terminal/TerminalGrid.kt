package io.github.code_akram.or2.terminal

import androidx.compose.ui.graphics.toArgb
import io.github.code_akram.or2.ffi.CellLink
import io.github.code_akram.or2.ffi.CellStyle
import io.github.code_akram.or2.ffi.CellWidth
import io.github.code_akram.or2.ffi.Scrollback
import io.github.code_akram.or2.ffi.TerminalCursor
import io.github.code_akram.or2.ffi.TerminalFrame
import io.github.code_akram.or2.ffi.TerminalModes
import io.github.code_akram.or2.ui.Or2Colors
import kotlin.math.floor

/**
 * The terminal's default background until the first frame: the theme's terminal colour, which is
 * the core's own default (core/terminal.rs; a test compares it with a real frame).
 */
val DefaultBackground: UInt = (Or2Colors.TerminalBackground.toArgb() and 0xFFFFFF).toUInt()

data class ResolvedCell(val text: String, val width: CellWidth, val style: CellStyle)
/** [links] are the row's OSC 8 hyperlinks (inclusive column runs), empty when none. */
data class ResolvedRow(val cells: List<ResolvedCell>, val wrapped: Boolean, val links: List<CellLink> = emptyList()) {
    internal val painted: PaintedRow by lazy(LazyThreadSafetyMode.NONE) { PaintedRow(cells) }
}
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
    /** What a swipe scrolls ([scrollRoute]); updated by every frame. */
    var modes = TerminalModes(false, false)
        private set
    var sequence = 0uL
        private set
    val hasGrid get() = rows.isNotEmpty()
    var needsFullFrame = false
        private set

    /** Returns whether drawing is needed (full snapshots redraw). [needsFullFrame] marks an unusable delta. */
    fun apply(frame: TerminalFrame): Boolean {
        // Every delta depends on the last taken state, including cell-only and metadata-only
        // deltas. A gap cannot be silently accepted and then used as the base for row moves.
        // Keep the broken-base latch until a self-contained full frame arrives.
        needsFullFrame = !frame.full && (needsFullFrame || !hasGrid || frame.sequence != sequence + 1u ||
            columns != frame.columns.toInt() || rows.size != frame.rows.toInt())
        if (frame.rowMoves.isNotEmpty()) {
            // Sources refer to the last *taken* state, never to replacements in this delta.
            needsFullFrame = needsFullFrame || frame.full || frame.sequence != sequence + 1u
            val destinations = BooleanArray(rows.size)
            var last = -1
            for (move in frame.rowMoves) {
                val index = move.index.toInt()
                if (index <= last || index !in rows.indices || move.previous.toInt() !in rows.indices) {
                    needsFullFrame = true
                    break
                }
                destinations[index] = true
                last = index
            }
            needsFullFrame = needsFullFrame || frame.changedRows.any {
                it.index.toInt() !in rows.indices || destinations[it.index.toInt()]
            }
        }
        if (needsFullFrame) return false
        var visibleChanged = frame.full ||
            cursor != frame.cursor || background != frame.background || scrollback != frame.scrollback || modes != frame.modes
        // Full rows are ascending and complete; deltas replace by index in one shallow copy.
        // Published row lists remain immutable so a selection keeps its frozen snapshot.
        if (frame.full) {
            rows = frame.changedRows.map { row ->
                ResolvedRow(row.cells.map { cell ->
                    ResolvedCell(cell.text, cell.width, frame.styles[cell.style.toInt()])
                }, row.wrapped, row.links)
            }
        } else if (frame.changedRows.isNotEmpty() || frame.rowMoves.isNotEmpty()) {
            val replacement = rows.toMutableList()
            for (move in frame.rowMoves) {
                val resolved = rows[move.previous.toInt()]
                val index = move.index.toInt()
                replacement[index] = resolved
                if (rows[index] != resolved) visibleChanged = true
            }
            for (row in frame.changedRows) {
                val resolved = ResolvedRow(row.cells.map { cell ->
                    ResolvedCell(cell.text, cell.width, frame.styles[cell.style.toInt()])
                }, row.wrapped, row.links)
                val index = row.index.toInt()
                if (rows[index] != resolved) {
                    replacement[index] = resolved
                    visibleChanged = true
                }
            }
            // Even equal-content copies preserve the source object's identity for the cache.
            if (visibleChanged || frame.rowMoves.isNotEmpty()) rows = replacement
        }
        columns = frame.columns.toInt()
        cursor = frame.cursor
        background = frame.background
        scrollback = frame.scrollback
        modes = frame.modes
        sequence = frame.sequence
        return visibleChanged
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
