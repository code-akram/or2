package io.github.code_akram.or2.terminal

import io.github.code_akram.or2.ffi.CellStyle
import io.github.code_akram.or2.ffi.CellWidth
import io.github.code_akram.or2.ffi.Scrollback
import io.github.code_akram.or2.ffi.TerminalModes
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
        1u, 1u, 2u, full, listOf(style(color)), rows, null, 0u, Scrollback(2u, 0u), TerminalModes(false, false))

    @Test fun deltaResolvesItsOwnTableWithoutRecolouringCachedRows() {
        val grid = TerminalGrid()
        assertTrue(grid.apply(frame(true, 0xff0000u, listOf(row(0, "A"), row(1, "B")))))
        assertTrue(grid.apply(frame(false, 0x0000ffu, listOf(row(1, "C"))).copy(sequence = 2u)))
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
        assertTrue(grid.apply(frame(false, 1u, listOf(row(0, "C"))).copy(sequence = 2u)))
        assertEquals(emptyList<Any>(), grid.rows[0].links)
    }

    @Test fun newViewRejectsDeltaAndMetadataOnlyDeltaKeepsRows() {
        val grid = TerminalGrid()
        assertFalse(grid.apply(frame(false, 1u, listOf(row(1, "C")))))
        grid.apply(frame(true, 2u, listOf(row(0, "A"), row(1, "B"))))
        assertTrue(grid.apply(frame(false, 3u, emptyList()).copy(sequence = 2u, scrollback = Scrollback(9u, 4u))))
        assertEquals(4uL, grid.scrollback.offset)
        assertEquals(2u, grid.rows[1].cells[0].style.foreground)
    }

    @Test fun everyFrameCarriesTheModesASwipeIsRoutedBy() {
        val grid = TerminalGrid()
        assertEquals(TerminalModes(false, false), grid.modes)
        grid.apply(frame(true, 2u, listOf(row(0, "A"), row(1, "B"))).copy(modes = TerminalModes(true, true)))
        assertEquals(TerminalModes(true, true), grid.modes)
        // A delta without rows (a mode change dirties none) still updates them.
        assertTrue(grid.apply(frame(false, 2u, emptyList()).copy(sequence = 2u)))
        assertEquals(TerminalModes(false, false), grid.modes)
    }

    @Test fun unchangedFramesAdvanceSequenceWithoutRequestingADrawOrSnapshot() {
        val grid = TerminalGrid()
        val full = frame(true, 2u, listOf(row(0, "A"), row(1, "B")))
        grid.apply(full)
        val frozen = grid.rows
        assertFalse(grid.apply(full.copy(full = false, changedRows = emptyList(), sequence = 2u)))
        assertSame(frozen, grid.rows)
        assertFalse(grid.needsFullFrame)
        assertEquals(2uL, grid.sequence)
        assertFalse(grid.apply(full.copy(full = false, sequence = 3u)))
        assertSame(frozen, grid.rows)
        assertTrue(grid.apply(full.copy(full = false, sequence = 4u, changedRows = listOf(row(1, "C")))))
        assertSame(frozen[0], grid.rows[0])
        assertEquals("B", frozen[1].cells[0].text)
        assertTrue(grid.apply(full.copy(full = false, sequence = 5u, changedRows = emptyList(), background = 7u)))
        assertTrue(grid.apply(full.copy(full = false, sequence = 6u, changedRows = emptyList(), cursor =
            io.github.code_akram.or2.ffi.TerminalCursor(0u, 0u, false,
                io.github.code_akram.or2.ffi.CursorShape.BAR, false, 2u))))
        assertFalse(grid.apply(full.copy(full = false, sequence = 7u, columns = 2u)))
        assertTrue(grid.needsFullFrame)
        assertEquals(1, grid.columns)
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

    @Test fun movesReuseResolvedObjectsSimultaneouslyWithoutUsingTheNewStyleTable() {
        val grid = TerminalGrid()
        val link = io.github.code_akram.or2.ffi.CellLink(0u, 0u, "https://example.org")
        grid.apply(frame(true, 0xff0000u, listOf(row(0, "A").copy(wrapped = true, links = listOf(link)), row(1, "B"))))
        val frozen = grid.rows
        val moves = listOf(io.github.code_akram.or2.ffi.TerminalRowMove(0u, 1u),
            io.github.code_akram.or2.ffi.TerminalRowMove(1u, 0u))
        assertTrue(grid.apply(frame(false, 0x0000ffu, emptyList()).copy(sequence = 2u, rowMoves = moves)))
        assertSame(frozen[1], grid.rows[0])
        assertSame(frozen[0], grid.rows[1])
        assertEquals(0xff0000u, grid.rows[1].cells[0].style.foreground)
        assertEquals(listOf(link), grid.rows[1].links)
        assertTrue(grid.rows[1].wrapped)
        assertEquals("A", frozen[0].cells[0].text)
        assertTrue(grid.apply(frame(false, 7u, listOf(row(1, "C"))).copy(sequence = 3u,
            rowMoves = listOf(io.github.code_akram.or2.ffi.TerminalRowMove(0u, 1u)))))
        assertSame(frozen[0], grid.rows[0])
        assertEquals(7u, grid.rows[1].cells[0].style.foreground)
    }

    @Test fun aCellOnlySequenceGapCannotBecomeTheBaseForLaterMoves() {
        val grid = TerminalGrid()
        val full = frame(true, 1u, listOf(row(0, "A"), row(1, "B")))
        grid.apply(full)
        val frozen = grid.rows
        // Another consumer took sequence 2, changing row 0. A metadata-only sequence 3
        // must not make our stale A/B grid look like a valid sequence-3 base.
        assertFalse(grid.apply(full.copy(full = false, sequence = 3u, changedRows = emptyList())))
        assertTrue(grid.needsFullFrame)
        assertEquals(1uL, grid.sequence)
        assertSame(frozen, grid.rows)
        assertFalse(grid.apply(full.copy(full = false, sequence = 2u, changedRows = emptyList())))
        assertTrue("Only a full snapshot can recover a broken base", grid.needsFullFrame)
        assertFalse(grid.apply(full.copy(full = false, sequence = 4u, changedRows = emptyList(),
            rowMoves = listOf(io.github.code_akram.or2.ffi.TerminalRowMove(1u, 0u)))))
        assertTrue(grid.apply(full.copy(sequence = 5u, changedRows = listOf(row(0, "C"), row(1, "D")))))
        assertFalse(grid.needsFullFrame)
        assertTrue(grid.apply(full.copy(full = false, sequence = 6u, changedRows = emptyList(),
            rowMoves = listOf(io.github.code_akram.or2.ffi.TerminalRowMove(1u, 0u)))))
        assertEquals("C", grid.rows[1].cells[0].text)
    }

    @Test fun invalidMovesRequestAFullSnapshotWithoutPartialApplication() {
        val full = frame(true, 1u, listOf(row(0, "A"), row(1, "B")))
        val move = io.github.code_akram.or2.ffi.TerminalRowMove(0u, 1u)
        for (invalid in listOf(full.copy(rowMoves = listOf(move)),
            full.copy(full = false, sequence = 3u, changedRows = emptyList(), rowMoves = listOf(move)),
            full.copy(full = false, sequence = 2u, rowMoves = listOf(move)),
            full.copy(full = false, sequence = 2u, changedRows = emptyList(), rowMoves = listOf(move, move)),
            full.copy(full = false, sequence = 2u, changedRows = emptyList(), rowMoves = listOf(move.copy(previous = 2u))))) {
            val grid = TerminalGrid()
            grid.apply(full)
            val before = grid.rows
            assertFalse(grid.apply(invalid))
            assertTrue(grid.needsFullFrame)
            assertSame(before, grid.rows)
            assertEquals(1uL, grid.sequence)
        }
    }
}
