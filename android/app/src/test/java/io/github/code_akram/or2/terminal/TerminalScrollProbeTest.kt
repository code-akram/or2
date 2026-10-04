package io.github.code_akram.or2.terminal

import io.github.code_akram.or2.RecordingListener
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.contractProbeSession
import org.junit.Assert.*
import org.junit.Test

class TerminalScrollProbeTest {
    @Test fun nativeScrollCrossesFfiAndReusesResolvedRowsAndFrozenSnapshots() {
        val listener = RecordingListener()
        contractProbeSession(56u, 47u, listener).use { session ->
            listener.awaitState<SessionState.Connected>()
            val grid = TerminalGrid()
            grid.apply(listener.awaitFrame(session))
            session.sendText("\u001bor2:scroll:start")
            grid.apply(listener.awaitFrame(session))
            val frozen = grid.rows
            repeat(4) {
                val previous = grid.rows
                session.sendText("\u001bor2:scroll:step")
                val frame = listener.awaitFrame(session)
                assertFalse(frame.full)
                assertEquals(1, frame.changedRows.size)
                assertEquals(46, frame.rowMoves.size)
                assertTrue(grid.apply(frame))
                assertFalse(grid.needsFullFrame)
                for (row in 0..45) assertSame(previous[row + 1], grid.rows[row])
            }
            assertEquals("00000000", frozen[0].cells.take(8).joinToString("") { it.text })
            session.requestFullFrame()
            val full = listener.awaitFrame(session)
            assertTrue(full.full)
            assertTrue(full.rowMoves.isEmpty())
            val before = grid.rows
            grid.apply(full)
            assertEquals(before, grid.rows)
            session.sendText("\u001bor2:scroll:stop")
            grid.apply(listener.awaitFrame(session))
            assertEquals("or2 contract probe", grid.rows[0].cells.joinToString("") { it.text }.trim())
            session.disconnect()
        }
    }
}
