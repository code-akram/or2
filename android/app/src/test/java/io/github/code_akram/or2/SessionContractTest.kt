package io.github.code_akram.or2

import io.github.code_akram.or2.ffi.CellWidth
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.CursorShape
import io.github.code_akram.or2.ffi.KeyInput
import io.github.code_akram.or2.ffi.KeyModifiers
import io.github.code_akram.or2.ffi.SessionException
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalFrame
import io.github.code_akram.or2.ffi.TerminalKey
import io.github.code_akram.or2.ffi.Underline
import io.github.code_akram.or2.ffi.ViewportScroll
import io.github.code_akram.or2.ffi.contractProbeSession
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Session lifecycle, callbacks, frames and input across the real FFI, driven by the contract
 * probe (a scripted session driver; no network). Host keys belong to the host
 * ([HostContractTest]).
 */
class SessionContractTest {
    private fun TerminalFrame.rowText(index: Int) =
        changedRows.single { it.index.toInt() == index }.cells.joinToString("") { it.text }.trimEnd()

    @Test
    fun emptyDimensionsAreRejectedSynchronously() {
        val listener = RecordingListener()
        assertThrows(SessionException.EmptyDimension::class.java) { contractProbeSession(0u, 5u, listener) }
        assertThrows(SessionException.EmptyDimension::class.java) { contractProbeSession(30u, 0u, listener) }
        listener.assertNoMoreStates()
    }

    @Test
    fun sessionRoundTripsStatesFramesAndInput() {
        val listener = RecordingListener()
        val testThread = Thread.currentThread()
        contractProbeSession(30u, 5u, listener).use { session ->
            listener.awaitState<SessionState.Connected>()

            val first = listener.awaitFrame(session)
            assertEquals(1uL, first.sequence)
            assertTrue(first.full)
            assertEquals(30.toUShort(), first.columns)
            assertEquals(5.toUShort(), first.rows)
            assertEquals((0 until 5).toList(), first.changedRows.map { it.index.toInt() })
            assertTrue(first.changedRows.all { it.cells.size == 30 })
            assertEquals("or2 contract probe", first.rowText(0))

            val cells = first.changedRows[1].cells
            assertEquals(listOf("R", "界", "", "😀", "", "e\u0301", "I"), cells.take(7).map { it.text })
            assertEquals(
                listOf(CellWidth.NARROW, CellWidth.WIDE, CellWidth.SPACER_TAIL, CellWidth.WIDE, CellWidth.SPACER_TAIL),
                cells.take(5).map { it.width },
            )
            val red = first.styles[cells[0].style.toInt()]
            assertEquals(0xff3333u, red.foreground)
            assertTrue(red.bold)
            val curly = first.styles[cells[5].style.toInt()]
            assertEquals(Underline.CURLY, curly.underline)
            assertEquals(0x3399ffu, curly.underlineColor)
            assertTrue(curly.italic)
            val inverse = first.styles[cells[6].style.toInt()]
            assertEquals(first.background, inverse.foreground)
            // Styles are deduplicated: the wide head, its tail and the blanks share one entry.
            assertEquals(cells[1].style, cells[2].style)
            assertEquals(cells[1].style, cells[29].style)
            assertEquals(4, first.styles.size)
            assertEquals(0x101018u, first.background)

            val cursor = first.cursor!!
            assertEquals(1.toUShort(), cursor.column)
            assertEquals(1.toUShort(), cursor.row)
            assertTrue(cursor.wide)
            assertEquals(CursorShape.BAR, cursor.shape)
            assertTrue(cursor.blinking)
            assertEquals(105uL, first.scrollback.totalRows)
            assertEquals(100uL, first.scrollback.offset)

            session.sendText("é\n")
            val text = listener.awaitFrame(session)
            assertEquals(2uL, text.sequence)
            assertFalse(text.full)
            assertEquals(listOf(2), text.changedRows.map { it.index.toInt() })
            assertEquals("text c3 a9 0d", text.rowText(2))

            // A submit echoes the typed text, then its Enter as the separate second write.
            session.submitText("é\nx")
            assertEquals("submit c3 a9 0d 78 | 0d", listener.awaitFrame(session).rowText(2))
            session.submitText("")
            assertEquals("submit | 0d", listener.awaitFrame(session).rowText(2))

            val ctrl = KeyModifiers(shift = false, ctrl = true, alt = false, meta = false)
            session.sendKey(KeyInput(TerminalKey.Character("c"), ctrl))
            assertEquals("key Character(\"c\")+ctrl", listener.awaitFrame(session).rowText(3))
            session.sendKey(KeyInput(TerminalKey.Function(12u), KeyModifiers(true, false, true, false)))
            assertEquals("key Function(12)+shift+alt", listener.awaitFrame(session).rowText(3))
            assertThrows(SessionException.InvalidKey::class.java) {
                session.sendKey(KeyInput(TerminalKey.Function(13u), ctrl))
            }
            assertThrows(SessionException.InvalidKey::class.java) {
                session.sendKey(KeyInput(TerminalKey.Character("\n"), ctrl))
            }

            // API 15: a tap's click crosses the FFI with its cell (the probe echoes it).
            session.mouseClick(7u, 3u)
            assertEquals("click 7 3", listener.awaitFrame(session).rowText(3))

            session.scroll(ViewportScroll.Delta(-10))
            val scrolled = listener.awaitFrame(session)
            assertTrue(scrolled.changedRows.isEmpty())
            assertEquals(90uL, scrolled.scrollback.offset)

            assertThrows(SessionException.EmptyDimension::class.java) { session.resize(0u, 5u) }
            session.resize(20u, 4u)
            val resized = listener.awaitFrame(session)
            assertTrue(resized.full)
            assertEquals(20.toUShort(), resized.columns)
            assertEquals(4, resized.changedRows.size)

            session.requestFullFrame()
            assertTrue(listener.awaitFrame(session).full)

            session.disconnect()
            val closed = listener.awaitState<SessionState.Closed>()
            assertEquals(CloseReason.Disconnected, closed.reason)
            assertEquals(closed, session.state())
            assertThrows(SessionException.Closed::class.java) { session.sendText("late") }
            assertThrows(SessionException.Closed::class.java) { session.submitText("late") }
            assertThrows(SessionException.Closed::class.java) { session.resize(10u, 10u) }
            session.disconnect() // Idempotent.
            listener.assertNoMoreStates()
            listener.assertNoFrameSignal()
        }
        assertFalse("callbacks never overlap", listener.overlapped)
        assertFalse("callbacks never run on the caller's thread", testThread in listener.callbackThreads)
    }

    @Test
    fun listenerExceptionsDoNotAffectTheSession() {
        val listener = RecordingListener(throwAfterRecording = true)
        contractProbeSession(40u, 6u, listener).use { session ->
            listener.awaitState<SessionState.Connected>()
            assertTrue(listener.awaitFrame(session).full)
            session.sendText("x")
            assertEquals("text 78", listener.awaitFrame(session).rowText(2))
            session.disconnect()
            listener.awaitState<SessionState.Closed>()
        }
    }

    @Test
    fun closingTheSessionObjectDisconnects() {
        val listener = RecordingListener()
        val session = contractProbeSession(40u, 6u, listener)
        listener.awaitState<SessionState.Connected>()
        session.close()
        assertEquals(CloseReason.Disconnected, listener.awaitState<SessionState.Closed>().reason)
    }
}
