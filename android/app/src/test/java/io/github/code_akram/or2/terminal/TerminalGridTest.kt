package io.github.code_akram.or2.terminal

import io.github.code_akram.or2.ffi.CellStyle
import io.github.code_akram.or2.ffi.CellWidth
import io.github.code_akram.or2.ffi.Scrollback
import io.github.code_akram.or2.ffi.TerminalCell
import io.github.code_akram.or2.ffi.TerminalFrame
import io.github.code_akram.or2.ffi.TerminalRow
import io.github.code_akram.or2.ffi.Underline
import org.junit.Assert.*
import org.junit.Test

class TerminalGridTest {
    @Test
    fun theKotlinDefaultBackgroundIsTheCoresDefault() {
        // core/terminal.rs sets this as the terminal's default background; the app's theme and a
        // grid before its first frame must agree with it, or the card around the terminal would
        // flash a different colour.
        val source = java.io.File("../../core/or2-core/src/terminal.rs").readText()
        val match = Regex("""DEFAULT_BACKGROUND: RgbColor = RgbColor \{\s*r: 0x(\w+),\s*g: 0x(\w+),\s*b: 0x(\w+),""").find(source)
        assertNotNull("DEFAULT_BACKGROUND not found in core/or2-core/src/terminal.rs", match)
        val (r, g, b) = match!!.destructured
        assertEquals(((r.toInt(16) shl 16) or (g.toInt(16) shl 8) or b.toInt(16)).toUInt(), DefaultBackground)
        assertEquals(DefaultBackground, TerminalGrid().background)
    }

    private fun style(color: UInt) = CellStyle(color, 0u, null, Underline.NONE, false, false, false, false, false)
    private fun row(index: Int, text: String) = TerminalRow(index.toUShort(), false,
        listOf(TerminalCell(text, CellWidth.NARROW, 0u)))
    private fun frame(full: Boolean, color: UInt, rows: List<TerminalRow>) = TerminalFrame(
        1u, 1u, 2u, full, listOf(style(color)), rows, null, 0u, Scrollback(2u, 0u))

    @Test fun deltaResolvesItsOwnTableWithoutRecolouringCachedRows() {
        val grid = TerminalGrid()
        assertTrue(grid.apply(frame(true, 0xff0000u, listOf(row(0, "A"), row(1, "B")))))
        assertTrue(grid.apply(frame(false, 0x0000ffu, listOf(row(1, "C")))))
        assertEquals(0xff0000u, grid.rows[0].cells[0].style.foreground)
        assertEquals(0x0000ffu, grid.rows[1].cells[0].style.foreground)
        assertEquals("C", grid.rows[1].cells[0].text)
        assertTrue(grid.apply(frame(true, 0x00ff00u, listOf(row(0, "D"), row(1, "E")))))
        assertEquals("D", grid.rows[0].cells[0].text)
        assertEquals(0x00ff00u, grid.rows[0].cells[0].style.foreground)
    }

    @Test fun rowLinksAreKeptWithTheirRowAndReplacedWithIt() {
        val grid = TerminalGrid()
        val link = io.github.code_akram.or2.ffi.CellLink(0u, 0u, "https://example.org")
        assertTrue(grid.apply(frame(true, 1u, listOf(row(0, "A").copy(links = listOf(link)), row(1, "B")))))
        assertEquals(listOf(link), grid.rows[0].links)
        assertEquals(emptyList<Any>(), grid.rows[1].links)
        assertTrue(grid.apply(frame(false, 1u, listOf(row(0, "C")))))
        assertEquals(emptyList<Any>(), grid.rows[0].links)
    }

    @Test fun newViewRejectsDeltaAndMetadataOnlyDeltaKeepsRows() {
        val grid = TerminalGrid()
        assertFalse(grid.apply(frame(false, 1u, listOf(row(1, "C")))))
        grid.apply(frame(true, 2u, listOf(row(0, "A"), row(1, "B"))))
        assertTrue(grid.apply(frame(false, 3u, emptyList()).copy(scrollback = Scrollback(9u, 4u))))
        assertEquals(4uL, grid.scrollback.offset)
        assertEquals(2u, grid.rows[1].cells[0].style.foreground)
    }

    @Test fun resizeFloorsCellsAndNeverSendsZero() {
        assertNull(gridSize(0, 20, 10f, 10f))
        assertNull(gridSize(9, 20, 10f, 10f))
        assertEquals(GridSize(3u, 2u), gridSize(39, 29, 10f, 10f))
        assertEquals(GridSize(3u, 1u), gridSize(39, 19, 10f, 10f))
        assertEquals(GridSize(65535u, 65535u), gridSize(Int.MAX_VALUE, Int.MAX_VALUE, 1f, 1f))
    }

    @Test fun rollingPercentilesDiscardOldSamples() {
        val timings = FrameTimings(3)
        listOf(99, 1, 2, 3).forEach { timings.record(it * 1_000_000L) }
        assertEquals(2.0, timings.percentile(50), 0.0)
        assertEquals(3.0, timings.percentile(95), 0.0)
        timings.clear()
        assertEquals(0, timings.count)
        timings.record(10_000_000L)
        assertEquals(10.0, timings.percentile(95), 0.0)
    }
}
