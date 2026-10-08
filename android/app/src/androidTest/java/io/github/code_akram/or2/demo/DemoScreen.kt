package io.github.code_akram.or2.demo

import io.github.code_akram.or2.ffi.CellStyle
import io.github.code_akram.or2.ffi.CellWidth
import io.github.code_akram.or2.ffi.CursorShape
import io.github.code_akram.or2.ffi.Scrollback
import io.github.code_akram.or2.ffi.TerminalCell
import io.github.code_akram.or2.ffi.TerminalCursor
import io.github.code_akram.or2.ffi.TerminalFrame
import io.github.code_akram.or2.ffi.TerminalModes
import io.github.code_akram.or2.ffi.TerminalRow
import io.github.code_akram.or2.ffi.Underline

/** Tokyo Night as the Rust core resolves it (core/terminal.rs), plus the accents the demo's programs use. */
internal object Tn {
    const val FG = 0xc0caf5u
    const val BG = 0x1a1b26u
    const val MUTED = 0x565f89u
    const val COMMENT = 0x737aa2u
    const val RED = 0xf7768eu
    const val GREEN = 0x9ece6au
    const val YELLOW = 0xe0af68u
    const val BLUE = 0x7aa2f7u
    const val MAGENTA = 0xbb9af7u
    const val TEAL = 0x73dacau
    const val PEACH = 0xff9e64u
    const val CLAUDE = 0xd77757u
    const val ADD_BG = 0x20303bu
    const val DEL_BG = 0x37222cu
    const val SELECT_BG = 0x283457u
}

/** A run of text in one style. Every character is one narrow cell: the demo never uses wide characters. */
internal data class Span(val text: String, val fg: UInt = Tn.FG, val bold: Boolean = false, val bg: UInt = Tn.BG, val italic: Boolean = false)

/** One logical line of a demo screen, laid out for the terminal's width (a box border fills it); long lines wrap. */
internal fun interface Line {
    fun spans(columns: Int): List<Span>
}

internal fun line(vararg spans: Span) = Line { spans.toList() }
internal fun text(text: String, fg: UInt = Tn.FG, bold: Boolean = false) = line(Span(text, fg, bold))
internal val blank = Line { emptyList() }

/** A full-width row of [bg] (a diff line, a selected row). */
internal fun filled(bg: UInt, vararg spans: Span) = Line { columns ->
    val used = spans.sumOf { it.text.length }
    spans.map { it.copy(bg = bg) } + Span(" ".repeat((columns - used).coerceAtLeast(0)), bg = bg)
}

/** A rounded box drawn in [color] around [rows], as wide as the terminal. */
internal fun box(color: UInt, vararg rows: List<Span>): List<Line> {
    val top = Line { c -> listOf(Span("╭" + "─".repeat((c - 2).coerceAtLeast(0)) + "╮", color)) }
    val bottom = Line { c -> listOf(Span("╰" + "─".repeat((c - 2).coerceAtLeast(0)) + "╯", color)) }
    return listOf(top) + rows.map { inner ->
        Line { c ->
            val width = (c - 4).coerceAtLeast(1)
            val clipped = clip(inner, width)
            val used = clipped.sumOf { it.text.length }
            listOf(Span("│ ", color)) + clipped + Span(" ".repeat((width - used).coerceAtLeast(0)) + " │", color)
        }
    } + bottom
}

private fun clip(spans: List<Span>, width: Int): List<Span> {
    var left = width
    val out = ArrayList<Span>()
    for (span in spans) {
        if (left <= 0) break
        val taken = span.text.take(left)
        out += span.copy(text = taken)
        left -= taken.length
    }
    return out
}

/**
 * What one program shows: its [body] (the output so far) and a [footer] that always follows it (an agent's input box).
 * Laid out like a terminal that has been running a while: the newest rows at the bottom, older ones scrolled away.
 * Mutated on the main thread; frames are read on the app's frame worker, so all access holds [lock].
 */
internal class DemoScreen(private val lock: Any, private val changed: () -> Unit) {
    private var body: List<Line> = emptyList()
    private var footer: List<Line> = emptyList()

    /** The cursor: the footer row (from its first row) and the column; null hides it. */
    private var cursor: Pair<Int, Int>? = null

    fun set(body: List<Line>, footer: List<Line> = emptyList(), cursor: Pair<Int, Int>? = null) {
        synchronized(lock) {
            this.body = body
            this.footer = footer
            this.cursor = cursor
        }
        changed()
    }

    fun append(vararg lines: Line) = append(lines.toList())

    fun append(lines: List<Line>, keep: Int = 400) {
        synchronized(lock) { body = (body + lines).takeLast(keep) }
        changed()
    }

    /** Replaces the last [count] body lines (a spinner's line, a progress line). */
    fun replaceLast(count: Int, vararg lines: Line) {
        synchronized(lock) { body = body.dropLast(count) + lines }
        changed()
    }

    fun setFooter(footer: List<Line>, cursor: Pair<Int, Int>?) {
        synchronized(lock) {
            this.footer = footer
            this.cursor = cursor
        }
        changed()
    }

    fun frame(columns: Int, rows: Int, sequence: ULong): TerminalFrame = synchronized(lock) {
        val bodyRows = body.flatMap { wrap(it.spans(columns), columns) }
        val footerRows = footer.flatMap { wrap(it.spans(columns), columns) }
        val all = bodyRows + footerRows
        val shown = all.takeLast(rows)
        val styles = LinkedHashMap<Triple<UInt, UInt, Int>, UInt>()
        fun style(span: Span): UInt {
            val flags = (if (span.bold) 1 else 0) or (if (span.italic) 2 else 0)
            return styles.getOrPut(Triple(span.fg, span.bg, flags)) { styles.size.toUInt() }
        }
        style(Span("")) // Index 0: the plain style.
        val changed = List(rows) { index ->
            val cells = ArrayList<TerminalCell>(columns)
            shown.getOrNull(index)?.forEach { (ch, span) -> cells += TerminalCell(ch.toString(), CellWidth.NARROW, style(span)) }
            while (cells.size < columns) cells += TerminalCell("", CellWidth.NARROW, 0u)
            TerminalRow(index.toUShort(), false, cells)
        }
        val cursorAt = cursor?.let { (footerRow, column) ->
            val absolute = bodyRows.size + footerRow
            val row = absolute - (all.size - shown.size)
            if (row in 0 until rows) TerminalCursor(column.coerceIn(0, columns - 1).toUShort(), row.toUShort(), false, CursorShape.BLOCK, false, Tn.FG) else null
        }
        TerminalFrame(
            sequence, columns.toUShort(), rows.toUShort(), true,
            styles.keys.map { (fg, bg, flags) ->
                CellStyle(fg, bg, null, Underline.NONE, flags and 1 != 0, flags and 2 != 0, false, false, false)
            },
            changed, cursorAt, Tn.BG, Scrollback(rows.toULong(), 0uL), TerminalModes(false, false),
        )
    }

    private fun wrap(spans: List<Span>, columns: Int): List<List<Pair<Char, Span>>> {
        val cells = spans.flatMap { span -> span.text.map { it to span } }
        if (cells.isEmpty()) return listOf(emptyList())
        return cells.chunked(columns.coerceAtLeast(1))
    }
}
