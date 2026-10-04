package io.github.code_akram.or2.terminal

import io.github.code_akram.or2.ffi.CellLink
import io.github.code_akram.or2.ffi.CellStyle
import io.github.code_akram.or2.ffi.CellWidth
import io.github.code_akram.or2.ffi.Underline
import org.junit.Assert.*
import org.junit.Test

class TerminalRowCacheTest {
    private val style = CellStyle(1u, 2u, null, Underline.NONE, false, false, false, false, false)
    private fun row(text: String) = ResolvedRow(listOf(ResolvedCell(text, CellWidth.NARROW, style)), false)

    @Test fun equalityUsesAllPaintedContentAndIgnoresLinksAndWrapping() {
        val original = row("a")
        assertEquals(original.painted, original.copy(wrapped = true,
            links = listOf(CellLink(0u, 0u, "https://example.org"))).painted)
        assertNotEquals(original.painted, row("b").painted)
        assertNotEquals(original.painted, ResolvedRow(listOf(original.cells[0].copy(width = CellWidth.WIDE)), false).painted)
        for (other in listOf(style.copy(bold = true), style.copy(italic = true), style.copy(faint = true),
            style.copy(foreground = 3u), style.copy(background = 3u), style.copy(underline = Underline.CURLY),
            style.copy(underlineColor = 3u), style.copy(strikethrough = true), style.copy(overline = true))) {
            assertNotEquals(original.painted, ResolvedRow(listOf(original.cells[0].copy(style = other)), false).painted)
        }
    }

    @Test fun movedAndDuplicateRowsHitAndEvictionAndClearReleaseEntries() {
        val released = mutableListOf<String>()
        val cache = TerminalRowCache<String> { released += it }
        cache.limit = 2
        fun get(text: String) = cache.getOrPut(row(text).painted, true) { text }
        assertEquals("a", get("a"))
        get("b")
        repeat(2) { assertEquals("a", get("a")) }
        get("c")
        assertEquals(listOf("b"), released)
        assertEquals(2L, cache.hits)
        assertEquals(3L, cache.misses)
        cache.limit = 1
        assertEquals(listOf("b", "a"), released)
        cache.clear()
        assertEquals(listOf("b", "a", "c"), released)
        assertEquals(0, cache.size)
        cache.resetCounters()
        assertEquals(0L, cache.hits)
        assertEquals(0L, cache.misses)
    }
}
