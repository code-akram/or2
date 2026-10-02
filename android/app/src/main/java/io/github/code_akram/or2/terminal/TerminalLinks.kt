package io.github.code_akram.or2.terminal

import io.github.code_akram.or2.ffi.CellWidth

/** The cells of one row a link covers, inclusive. */
data class LinkSpan(val row: Int, val columns: IntRange)

/** A link under a tap: what to open, and the cells it covers (for the tap's underline). */
data class TerminalLink(val uri: String, val spans: List<LinkSpan>)

/**
 * Finds the link at a cell of the displayed rows: an OSC 8 hyperlink ([ResolvedRow.links]) first, then a
 * plain-text URL. Text URLs are read across soft-wrapped rows ([ResolvedRow.wrapped]), so one broken over
 * lines opens whole. Only `http` and `https` open, for OSC 8 links too: a `file:` link names a file on
 * the host, and `intent:` or `javascript:` must never come from a remote program.
 */
object TerminalLinks {
    private val schemes = setOf("http", "https")

    /** A URL starts at a word boundary; the match stops at whitespace, quotes and angle brackets. */
    private val url = Regex("""\bhttps?://[^\s<>"'`]+""", RegexOption.IGNORE_CASE)

    /** Never the last character of a URL in text: the sentence's punctuation, not the link's. */
    private const val TRAILING = ".,;:!?*"
    private val closers = mapOf(')' to '(', ']' to '[', '}' to '{')

    fun at(rows: List<ResolvedRow>, position: CellPosition): TerminalLink? {
        val row = rows.getOrNull(position.row) ?: return null
        if (position.column !in row.cells.indices) return null
        val column = if (row.cells[position.column].width == CellWidth.SPACER_TAIL) position.column - 1 else position.column
        row.links.firstOrNull { column in it.startColumn.toInt()..it.endColumn.toInt() }?.let { link ->
            if (allowed(link.uri)) {
                return TerminalLink(link.uri, listOf(LinkSpan(position.row, link.startColumn.toInt()..link.endColumn.toInt())))
            }
        }
        return textLink(rows, position.row, column)
    }

    /** Whether [uri] may be opened: `http` or `https` with something after the `//`. */
    fun allowed(uri: String): Boolean {
        val colon = uri.indexOf(':')
        if (colon <= 0 || uri.substring(0, colon).lowercase() !in schemes) return false
        return uri.startsWith("//", colon + 1) && uri.length > colon + 3
    }

    private fun textLink(rows: List<ResolvedRow>, row: Int, column: Int): TerminalLink? {
        var first = row
        while (first > 0 && rows[first - 1].wrapped) first--
        var last = row
        while (last < rows.size - 1 && rows[last].wrapped) last++
        // The logical line's text, with the cell each character came from.
        val text = StringBuilder()
        val cellRows = ArrayList<Int>()
        val cellColumns = ArrayList<Int>()
        var tapped = -1
        for (index in first..last) {
            rows[index].cells.forEachIndexed { cellColumn, cell ->
                if (cell.width == CellWidth.SPACER_TAIL) return@forEachIndexed
                if (index == row && cellColumn == column) tapped = text.length
                val chars = cell.text.ifEmpty { " " }
                text.append(chars)
                repeat(chars.length) {
                    cellRows += index
                    cellColumns += cellColumn
                }
            }
        }
        if (tapped < 0) return null
        for (match in url.findAll(text)) {
            val start = match.range.first
            val end = start + trimmed(match.value).length // Exclusive.
            if (tapped !in start until end) continue
            val uri = text.substring(start, end)
            if (!allowed(uri)) return null
            val spans = (start until end).groupBy { cellRows[it] }.map { (spanRow, indices) ->
                val lastColumn = cellColumns[indices.last()]
                val wide = rows[spanRow].cells[lastColumn].width == CellWidth.WIDE
                LinkSpan(spanRow, cellColumns[indices.first()]..lastColumn + if (wide) 1 else 0)
            }
            return TerminalLink(uri, spans)
        }
        return null
    }

    /**
     * [candidate] without what follows a URL in prose: trailing punctuation, and a closing bracket that
     * closes one opened before the URL rather than in it (`(see https://example.org/a_(b))` keeps
     * `a_(b)`).
     */
    internal fun trimmed(candidate: String): String {
        var end = candidate.length
        while (end > 0) {
            val last = candidate[end - 1]
            val opener = closers[last]
            end = when {
                last in TRAILING -> end - 1
                opener != null && candidate.take(end).count { it == last } > candidate.take(end).count { it == opener } -> end - 1
                else -> break
            }
        }
        return candidate.take(end)
    }
}
