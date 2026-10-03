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

/** Catppuccin Mocha terminal colours as the Rust core resolves them (core/terminal.rs). */
private object Mocha {
    const val FG = 0xcdd6f4u
    const val BG = 0x1e1e2eu
    const val MUTED = 0x6c7086u
    const val RED = 0xf38ba8u
    const val GREEN = 0xa6e3a1u
    const val YELLOW = 0xf9e2afu
    const val BLUE = 0x89b4fau
    const val MAGENTA = 0xf5c2e7u
    const val TEAL = 0x94e2d5u
    const val PEACH = 0xfab387u
}

private data class Span(val text: String, val fg: UInt = Mocha.FG, val bg: UInt = Mocha.BG, val bold: Boolean = false, val underline: Boolean = false)

private fun line(vararg spans: Span) = spans.toList()
private fun plain(text: String) = line(Span(text))
private fun muted(text: String) = line(Span(text, Mocha.MUTED))

/** A coding-agent session in a shell, wrapped to [columns]: what the gallery's terminals show. */
private fun demoLines(columns: Int): List<List<Span>> {
    val inner = (columns - 4).coerceAtLeast(10)
    val bar = "─".repeat(inner + 2)
    fun boxed(vararg spans: Span): List<Span> {
        val used = spans.sumOf { it.text.length }
        return listOf(Span("│ ", Mocha.PEACH)) + spans.toList() + Span(" ".repeat((inner - used).coerceAtLeast(0)) + " │", Mocha.PEACH)
    }
    // Earlier output, so a tall terminal is full like a real session.
    val history = (1..28).flatMap { i ->
        listOf(
            line(Span("test ", Mocha.FG), Span("terminal::tests::case_$i", Mocha.MUTED), Span(" ... ", Mocha.FG), Span("ok", Mocha.GREEN)),
            if (i % 7 == 0) line(Span("warning: ", Mocha.YELLOW, bold = true), Span("unused import `Palette` in tests", Mocha.FG)) else
                line(Span("  \u23bf ", Mocha.MUTED), Span("Read", Mocha.TEAL), Span("(core/or2-core/src/part_$i.rs)", Mocha.MUTED)),
        )
    }
    return history + listOf(
        line(Span("  claude code ", Mocha.BLUE, bold = true), Span("v2.1.4", Mocha.MUTED)),
        muted("  ~/code/or2 · m2/ui-polish"),
        plain(""),
        line(Span("● ", Mocha.GREEN), Span("I'll match the toolbar to docs/ui.md.")),
        line(Span("  ⎿ ", Mocha.MUTED), Span("Read", Mocha.TEAL), Span("(ui/Theme.kt)", Mocha.MUTED)),
        line(Span("  ⎿ ", Mocha.MUTED), Span("Edit", Mocha.TEAL), Span("(terminal/TerminalChrome.kt)", Mocha.MUTED)),
        line(Span("      + ", Mocha.GREEN), Span("val Key = TextStyle(fontFamily = mono)", Mocha.GREEN)),
        line(Span("      - ", Mocha.RED), Span("val Key = TextStyle(fontSize = 14.sp)", Mocha.RED)),
        plain(""),
        line(Span("╭" + bar + "╮", Mocha.PEACH)),
        boxed(Span("Do you want to apply this edit?", Mocha.FG, bold = true)),
        boxed(Span("❯ 1. Yes", Mocha.BLUE)),
        boxed(Span("  2. Yes, and don't ask again", Mocha.FG)),
        boxed(Span("  3. No, tell Claude what to do instead", Mocha.FG)),
        line(Span("╰" + bar + "╯", Mocha.PEACH)),
        plain(""),
        line(Span("dev", Mocha.GREEN), Span("@", Mocha.MUTED), Span("workstation ", Mocha.BLUE), Span("~/code/or2 ", Mocha.MAGENTA), Span("(m2/ui-polish)", Mocha.YELLOW)),
        line(Span("$ ", Mocha.GREEN), Span("cargo test -p or2-core", Mocha.FG)),
        line(Span("   Compiling ", Mocha.GREEN, bold = true), Span("or2-core v0.1.0", Mocha.FG)),
        line(Span("test result: ", Mocha.FG), Span("ok", Mocha.GREEN), Span(". 23 passed; 0 failed", Mocha.FG)),
        line(Span("✗ ", Mocha.RED), Span("1 warning emitted", Mocha.YELLOW)),
        line(Span("dev", Mocha.GREEN), Span("@", Mocha.MUTED), Span("workstation ", Mocha.BLUE), Span("~/code/or2 ", Mocha.MAGENTA), Span("(m2/ui-polish)", Mocha.YELLOW)),
        line(Span("❯ ", Mocha.GREEN)),
    )
}

/** Build output that fills every cell of every row, so whatever floats over the terminal sits on text. */
private fun denseLines(columns: Int, rows: Int): List<List<Span>> {
    val words = "Compiling or2-core terminal::tests::wheel ok warning unused import Palette Finished dev profile " +
        "running 512 tests test result ok passed failed ignored measured filtered out "
    return List(rows) { row ->
        val start = (row * 17) % words.length
        val text = (words.repeat(columns / words.length + 2)).substring(start, start + columns)
        val colour = listOf(Mocha.FG, Mocha.MUTED, Mocha.GREEN, Mocha.TEAL, Mocha.YELLOW)[row % 5]
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
    style(Mocha.FG, Mocha.BG, 0) // index 0 is the plain style
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
        TerminalCursor(promptColumn.toUShort(), (rows - 1).toUShort(), false, CursorShape.BLOCK, false, Mocha.BLUE),
        Mocha.BG, Scrollback((rows * 3).toULong(), (rows * 2).toULong()), TerminalModes(mouseTracking, mouseTracking),
    )
}
