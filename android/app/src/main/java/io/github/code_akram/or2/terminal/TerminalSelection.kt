package io.github.code_akram.or2.terminal

import io.github.code_akram.or2.ffi.CellWidth

data class CellPosition(val column: Int, val row: Int)

/** Drag coordinates belong to the displayed snapshot, even if a larger live frame arrives. */
fun TerminalGrid.position(x: Float, y: Float, cellWidth: Float, cellHeight: Float, selection: TerminalSelection?): CellPosition? {
    val columns = selection?.columns ?: this.columns
    val rowCount = selection?.rows?.size ?: rows.size
    if (columns == 0 || rowCount == 0) return null
    return CellPosition((x / cellWidth).toInt().coerceIn(0, columns - 1),
        (y / cellHeight).toInt().coerceIn(0, rowCount - 1))
}

/** Captures the displayed rows, so arriving output cannot silently change what Copy copies. */
class TerminalSelection(
    val rows: List<ResolvedRow>, val columns: Int, val anchor: CellPosition,
    private val initialEnd: CellPosition = anchor,
) {
    var end = initialEnd

    companion object {
        fun word(rows: List<ResolvedRow>, columns: Int, position: CellPosition): TerminalSelection {
            val cells = rows[position.row].cells
            fun blank(column: Int) = cells[column].width != CellWidth.SPACER_TAIL && cells[column].text.isBlank()
            var first = position.column
            var last = position.column
            if (!blank(first)) {
                while (first > 0 && !blank(first - 1)) first--
                while (last < columns - 1 && !blank(last + 1)) last++
            }
            return TerminalSelection(rows, columns, CellPosition(first, position.row), CellPosition(last, position.row))
        }
    }

    fun range(row: Int): IntRange? {
        fun index(position: CellPosition) = position.row * columns + position.column
        val first = minOf(index(anchor), index(end))
        val last = maxOf(index(initialEnd), index(end)) // Drag extends, rather than truncates, the initial word.
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
