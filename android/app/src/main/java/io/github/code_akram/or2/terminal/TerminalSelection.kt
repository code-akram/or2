package io.github.code_akram.or2.terminal

import io.github.code_akram.or2.ffi.CellWidth

data class CellPosition(val column: Int, val row: Int)

/** Captures the displayed rows, so arriving output cannot silently change what Copy copies. */
class TerminalSelection(val rows: List<ResolvedRow>, val columns: Int, val anchor: CellPosition) {
    var end = anchor

    fun range(row: Int): IntRange? {
        fun index(position: CellPosition) = position.row * columns + position.column
        val first = minOf(index(anchor), index(end))
        val last = maxOf(index(anchor), index(end))
        if (row !in first / columns..last / columns) return null
        var startColumn = if (row == first / columns) first % columns else 0
        var endColumn = if (row == last / columns) last % columns else columns - 1
        if (rows[row].cells[startColumn].width == CellWidth.SPACER_TAIL) startColumn--
        if (rows[row].cells[endColumn].width == CellWidth.WIDE) endColumn++
        return startColumn..endColumn
    }

    fun text(): String = buildString {
        rows.forEachIndexed { rowIndex, row ->
            val range = range(rowIndex) ?: return@forEachIndexed
            append(range.joinToString("") { column ->
                val cell = row.cells[column]
                when {
                    cell.width == CellWidth.SPACER_TAIL -> ""
                    cell.text.isEmpty() -> " "
                    else -> cell.text
                }
            }.trimEnd(' '))
            if (!row.wrapped && range(rowIndex + 1) != null) append('\n')
        }
    }
}
