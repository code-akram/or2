package io.github.code_akram.or2.terminal

import io.github.code_akram.or2.ffi.CellStyle
import io.github.code_akram.or2.ffi.CellWidth
import io.github.code_akram.or2.ffi.Underline
import org.junit.Assert.assertEquals
import org.junit.Test

class TerminalSelectionTest {
    private val style = CellStyle(1u, 0u, null, Underline.NONE, false, false, false, false, false)
    private fun row(text: String, wrapped: Boolean = false) = ResolvedRow(
        text.map { ResolvedCell(if (it == ' ') "" else "$it", CellWidth.NARROW, style) }, wrapped)

    @Test fun selectionJoinsWrappedRowsAndTrimsBlanksButKeepsInteriorSpaces() {
        val rows = listOf(row("ab  ", true), row("c d "), row("ef  "))
        val selection = TerminalSelection(rows, 4, CellPosition(0, 0))
        selection.end = CellPosition(3, 2)
        assertEquals("abc d\nef", selection.text())
        val reverse = TerminalSelection(rows, 4, CellPosition(1, 2))
        reverse.end = CellPosition(1, 0)
        assertEquals("bc d\nef", reverse.text())
    }

    @Test fun partialRowsAndWideTailSelectionKeepWholeGraphemes() {
        val wide = ResolvedRow(listOf(
            ResolvedCell("x", CellWidth.NARROW, style),
            ResolvedCell("界", CellWidth.WIDE, style),
            ResolvedCell("", CellWidth.SPACER_TAIL, style),
            ResolvedCell("e\u0301", CellWidth.NARROW, style)), false)
        val selection = TerminalSelection(listOf(wide, row("abcd")), 4, CellPosition(2, 0))
        assertEquals("界", selection.text())
        assertEquals(1..2, selection.range(0))
        selection.end = CellPosition(1, 1)
        assertEquals("界e\u0301\nab", selection.text())
    }
}
