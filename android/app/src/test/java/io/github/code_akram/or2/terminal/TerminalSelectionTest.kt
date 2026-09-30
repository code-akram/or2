package io.github.code_akram.or2.terminal

import io.github.code_akram.or2.ffi.CellStyle
import io.github.code_akram.or2.ffi.CellWidth
import io.github.code_akram.or2.ffi.Scrollback
import io.github.code_akram.or2.ffi.TerminalCell
import io.github.code_akram.or2.ffi.TerminalFrame
import io.github.code_akram.or2.ffi.TerminalRow
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

    @Test fun wordSelectionIncludesWideTailsAndCombiningMarksThenExtendsInEitherDirection() {
        val cells = listOf(
            ResolvedCell("a", CellWidth.NARROW, style), ResolvedCell("", CellWidth.NARROW, style),
            ResolvedCell("界", CellWidth.WIDE, style), ResolvedCell("", CellWidth.SPACER_TAIL, style),
            ResolvedCell("e\u0301", CellWidth.NARROW, style), ResolvedCell("!", CellWidth.NARROW, style),
            ResolvedCell(" ", CellWidth.NARROW, style), ResolvedCell("z", CellWidth.NARROW, style),
        )
        val rows = listOf(ResolvedRow(cells, false))
        val selection = TerminalSelection.word(rows, 8, CellPosition(3, 0)) // Finger is on the wide tail.
        assertEquals(2..5, selection.range(0))
        assertEquals("界e\u0301!", selection.text())
        selection.end = CellPosition(4, 0) // Moving inside the word must not truncate it.
        assertEquals("界e\u0301!", selection.text())
        selection.end = CellPosition(7, 0)
        assertEquals("界e\u0301! z", selection.text())
        selection.end = CellPosition(0, 0)
        assertEquals("a 界e\u0301!", selection.text())
        assertEquals("", TerminalSelection.word(rows, 8, CellPosition(1, 0)).text())
        assertEquals("z", TerminalSelection.word(rows, 8, CellPosition(7, 0)).text())
    }

    @Test fun dragAfterLargerFullFrameStillClampsAndCopiesTheSmallSnapshot() {
        fun frame(lines: List<String>) = TerminalFrame(1u, lines[0].length.toUShort(), lines.size.toUShort(),
            true, listOf(style), lines.mapIndexed { index, line ->
                TerminalRow(index.toUShort(), false, line.map { TerminalCell(it.toString(), CellWidth.NARROW, 0u) })
            }, null, 0u, Scrollback(lines.size.toULong(), 0u))
        val grid = TerminalGrid()
        grid.apply(frame(listOf("ab", "cd")))
        val selection = TerminalSelection(grid.rows, grid.columns, CellPosition(1, 0))
        grid.apply(frame(listOf("WXYZ", "1234", "5678", "9ABC")))
        assertEquals(CellPosition(3, 3), grid.position(100f, 100f, 10f, 10f, null))
        selection.end = grid.position(100f, 100f, 10f, 10f, selection)!!
        assertEquals(CellPosition(1, 1), selection.end)
        assertEquals("b\ncd", selection.text())
        assertEquals(CellPosition(0, 0), grid.position(-10f, -20f, 10f, 10f, selection))
    }
}
