package io.github.code_akram.or2.terminal

import io.github.code_akram.or2.ffi.CellStyle
import io.github.code_akram.or2.ffi.CellWidth
import io.github.code_akram.or2.ffi.CursorShape
import io.github.code_akram.or2.ffi.Scrollback
import io.github.code_akram.or2.ffi.TerminalCell
import io.github.code_akram.or2.ffi.TerminalCursor
import io.github.code_akram.or2.ffi.TerminalFrame
import io.github.code_akram.or2.ffi.TerminalRow
import io.github.code_akram.or2.ffi.Underline

/** Local, debug-only visual fixtures augment the native probe without changing its contract. */
fun terminalVisualFrame(columns: UShort, rows: UShort, shape: CursorShape): TerminalFrame {
    val plain = CellStyle(0xcdd6f4u, 0x1e1e2eu, null, Underline.NONE, false, false, false, false, false)
    val styles = listOf(plain, plain.copy(bold = true, foreground = 0xf38ba8u),
        plain.copy(italic = true), plain.copy(faint = true),
        plain.copy(strikethrough = true), plain.copy(overline = true)) +
        Underline.entries.filter { it != Underline.NONE }.map { plain.copy(underline = it, underlineColor = 0x89b4fau) } +
        plain.copy(foreground = 0x1e1e2eu, background = 0xcdd6f4u)
    val labels = listOf("Canvas styles (debug fixture)", "Bold foreground", "Italic foreground",
        "Faint foreground", "Strikethrough", "Overline", "Single underline", "Double underline",
        "Curly underline", "Dotted underline", "Dashed underline", "", "Resolved FG / BG")
    val changed = List(rows.toInt()) { row ->
        val style = when (row) {
            in 1..10 -> row.toUInt()
            12 -> 11u
            else -> 0u
        }
        val cells = MutableList(columns.toInt()) { TerminalCell("", CellWidth.NARROW, style) }
        labels.getOrNull(row)?.take(columns.toInt())?.forEachIndexed { column, char ->
            cells[column] = TerminalCell(char.toString(), CellWidth.NARROW, style)
        }
        if (row == 11 && columns >= 7u) {
            listOf("x", "界", "", "😀", "", "e\u0301", "!").forEachIndexed { column, text ->
                val width = when (column) {
                    1, 3 -> CellWidth.WIDE
                    2, 4 -> CellWidth.SPACER_TAIL
                    else -> CellWidth.NARROW
                }
                cells[column] = TerminalCell(text, width, 0u)
            }
        }
        TerminalRow(row.toUShort(), false, cells)
    }
    return TerminalFrame(1u, columns, rows, true, styles, changed,
        if (rows > 11u && columns >= 3u) TerminalCursor(1u, 11u, true, shape, false, 0x89b4fau) else null,
        0x1e1e2eu, Scrollback(rows.toULong(), 0u))
}

/** Dense all-row update; frame production is excluded from measured apply/record durations. */
fun terminalStressFrame(columns: UShort, rows: UShort, sequence: ULong): TerminalFrame {
    val styles = listOf(
        CellStyle(0xcdd6f4u, 0x1e1e2eu, null, Underline.NONE, false, false, false, false, false),
        CellStyle(0x89b4fau, 0x313244u, null, Underline.NONE, true, false, false, false, false),
    )
    val alphabet = "abcdefghijklmnopqrstuvwxyz0123456789"
    return TerminalFrame(sequence, columns, rows, true, styles, List(rows.toInt()) { row ->
        TerminalRow(row.toUShort(), false, List(columns.toInt()) { column ->
            TerminalCell(alphabet[(column + row + (sequence % alphabet.length.toULong()).toInt()) % alphabet.length].toString(),
                CellWidth.NARROW, (row % 2).toUInt())
        })
    }, null, 0x1e1e2eu, Scrollback(rows.toULong(), 0u))
}
