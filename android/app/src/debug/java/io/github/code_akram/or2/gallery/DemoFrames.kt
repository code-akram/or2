package io.github.code_akram.or2.gallery

import io.github.code_akram.or2.ffi.CellStyle
import io.github.code_akram.or2.ffi.CellWidth
import io.github.code_akram.or2.ffi.CursorShape
import io.github.code_akram.or2.ffi.Scrollback
import io.github.code_akram.or2.ffi.TerminalModes
import io.github.code_akram.or2.ffi.TerminalCell
import io.github.code_akram.or2.ffi.TerminalCursor
import io.github.code_akram.or2.ffi.TerminalFrame
import io.github.code_akram.or2.ffi.TerminalRow
import io.github.code_akram.or2.ffi.Underline

/** Tokyo Night terminal colours as the Rust core resolves them (core/terminal.rs), with its comment, teal and orange for the 24-bit spans. */
private object TokyoNight {
    const val FG = 0xc0caf5u
    const val BG = 0x1a1b26u
    const val MUTED = 0x565f89u
    const val RED = 0xf7768eu
    const val GREEN = 0x9ece6au
    const val YELLOW = 0xe0af68u
    const val BLUE = 0x7aa2f7u
    const val MAGENTA = 0xbb9af7u
    const val TEAL = 0x73dacau
    const val PEACH = 0xff9e64u
}

private data class Span(val text: String, val fg: UInt = TokyoNight.FG, val bg: UInt = TokyoNight.BG, val bold: Boolean = false, val underline: Boolean = false)

private fun line(vararg spans: Span) = spans.toList()
private fun plain(text: String) = line(Span(text))
private fun muted(text: String) = line(Span(text, TokyoNight.MUTED))

/** A coding-agent session in a shell, wrapped to [columns]: what the gallery's terminals show. */
private fun demoLines(columns: Int): List<List<Span>> {
    val inner = (columns - 4).coerceAtLeast(10)
    val bar = "─".repeat(inner + 2)
    fun boxed(vararg spans: Span): List<Span> {
        val used = spans.sumOf { it.text.length }
        return listOf(Span("│ ", TokyoNight.PEACH)) + spans.toList() + Span(" ".repeat((inner - used).coerceAtLeast(0)) + " │", TokyoNight.PEACH)
    }
    // Earlier output, so a tall terminal is full like a real session.
    val history = (1..28).flatMap { i ->
        listOf(
            line(Span("test ", TokyoNight.FG), Span("terminal::tests::case_$i", TokyoNight.MUTED), Span(" ... ", TokyoNight.FG), Span("ok", TokyoNight.GREEN)),
            if (i % 7 == 0) line(Span("warning: ", TokyoNight.YELLOW, bold = true), Span("unused import `Palette` in tests", TokyoNight.FG)) else
                line(Span("  \u23bf ", TokyoNight.MUTED), Span("Read", TokyoNight.TEAL), Span("(core/or2-core/src/part_$i.rs)", TokyoNight.MUTED)),
        )
    }
    return history + listOf(
        line(Span("  claude code ", TokyoNight.BLUE, bold = true), Span("v2.1.4", TokyoNight.MUTED)),
        muted("  ~/code/or2 · m2/ui-polish"),
        plain(""),
        line(Span("● ", TokyoNight.GREEN), Span("I'll match the toolbar to docs/ui.md.")),
        line(Span("  ⎿ ", TokyoNight.MUTED), Span("Read", TokyoNight.TEAL), Span("(ui/Theme.kt)", TokyoNight.MUTED)),
        line(Span("  ⎿ ", TokyoNight.MUTED), Span("Edit", TokyoNight.TEAL), Span("(terminal/TerminalChrome.kt)", TokyoNight.MUTED)),
        line(Span("      + ", TokyoNight.GREEN), Span("val Key = TextStyle(fontFamily = mono)", TokyoNight.GREEN)),
        line(Span("      - ", TokyoNight.RED), Span("val Key = TextStyle(fontSize = 14.sp)", TokyoNight.RED)),
        plain(""),
        line(Span("╭" + bar + "╮", TokyoNight.PEACH)),
        boxed(Span("Do you want to apply this edit?", TokyoNight.FG, bold = true)),
        boxed(Span("❯ 1. Yes", TokyoNight.BLUE)),
        boxed(Span("  2. Yes, and don't ask again", TokyoNight.FG)),
        boxed(Span("  3. No, tell Claude what to do instead", TokyoNight.FG)),
        line(Span("╰" + bar + "╯", TokyoNight.PEACH)),
        plain(""),
        line(Span("dev", TokyoNight.GREEN), Span("@", TokyoNight.MUTED), Span("workstation ", TokyoNight.BLUE), Span("~/code/or2 ", TokyoNight.MAGENTA), Span("(m2/ui-polish)", TokyoNight.YELLOW)),
        line(Span("$ ", TokyoNight.GREEN), Span("cargo test -p or2-core", TokyoNight.FG)),
        line(Span("   Compiling ", TokyoNight.GREEN, bold = true), Span("or2-core v0.1.0", TokyoNight.FG)),
        line(Span("test result: ", TokyoNight.FG), Span("ok", TokyoNight.GREEN), Span(". 23 passed; 0 failed", TokyoNight.FG)),
        line(Span("✗ ", TokyoNight.RED), Span("1 warning emitted", TokyoNight.YELLOW)),
        line(Span("dev", TokyoNight.GREEN), Span("@", TokyoNight.MUTED), Span("workstation ", TokyoNight.BLUE), Span("~/code/or2 ", TokyoNight.MAGENTA), Span("(m2/ui-polish)", TokyoNight.YELLOW)),
        line(Span("❯ ", TokyoNight.GREEN)),
    )
}

/** Build output that fills every cell of every row, so whatever floats over the terminal sits on text. */
private fun denseLines(columns: Int, rows: Int): List<List<Span>> {
    val words = "Compiling or2-core terminal::tests::wheel ok warning unused import Palette Finished dev profile " +
        "running 512 tests test result ok passed failed ignored measured filtered out "
    return List(rows) { row ->
        val start = (row * 17) % words.length
        val text = (words.repeat(columns / words.length + 2)).substring(start, start + columns)
        val colour = listOf(TokyoNight.FG, TokyoNight.MUTED, TokyoNight.GREEN, TokyoNight.TEAL, TokyoNight.YELLOW)[row % 5]
        line(Span(text, colour))
    }
}

/**
 * A full frame of [demoLines] bottom-aligned in [columns] x [rows], the cursor on the prompt.
 * Produced without the native core so the gallery can draw it anywhere, scaled or not. [dense]
 * fills every cell with text instead; [mouseTracking] is the frame's `TerminalModes.mouse_tracking`
 * (herdr, tmux with `mouse on`).
 */
fun terminalDemoFrame(columns: Int, rows: Int, sequence: ULong = 1u, dense: Boolean = false, mouseTracking: Boolean = false): TerminalFrame {
    val lines = (if (dense) denseLines(columns, rows) else demoLines(columns)).takeLast(rows)
    val blank = rows - lines.size
    val styleIndex = LinkedHashMap<Triple<UInt, UInt, Int>, UInt>()
    fun style(fg: UInt, bg: UInt, flags: Int): UInt = styleIndex.getOrPut(Triple(fg, bg, flags)) { styleIndex.size.toUInt() }
    style(TokyoNight.FG, TokyoNight.BG, 0) // index 0 is the plain style
    val changed = List(rows) { rowIndex ->
        val spans = lines.getOrNull(rowIndex - blank).orEmpty()
        val cells = ArrayList<TerminalCell>(columns)
        for (span in spans) {
            val s = style(span.fg, span.bg, (if (span.bold) 1 else 0) or (if (span.underline) 2 else 0))
            for (ch in span.text) {
                if (cells.size >= columns) break
                cells += TerminalCell(ch.toString(), CellWidth.NARROW, s)
            }
        }
        while (cells.size < columns) cells += TerminalCell("", CellWidth.NARROW, 0u)
        TerminalRow(rowIndex.toUShort(), false, cells)
    }
    val styles = styleIndex.keys.map { (fg, bg, flags) ->
        CellStyle(fg, bg, null, if (flags and 2 != 0) Underline.SINGLE else Underline.NONE, flags and 1 != 0, false, false, false, false)
    }
    val promptColumn = 2
    return TerminalFrame(
        sequence, columns.toUShort(), rows.toUShort(), true, styles, changed,
        TerminalCursor(promptColumn.toUShort(), (rows - 1).toUShort(), false, CursorShape.BLOCK, false, TokyoNight.BLUE),
        TokyoNight.BG, Scrollback((rows * 3).toULong(), (rows * 2).toULong()), TerminalModes(mouseTracking, mouseTracking),
    )
}
