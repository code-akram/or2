package io.github.code_akram.or2.terminal

import io.github.code_akram.or2.ffi.CellStyle
import io.github.code_akram.or2.ffi.CellWidth
import io.github.code_akram.or2.ffi.KeyInput
import io.github.code_akram.or2.ffi.Scrollback
import io.github.code_akram.or2.ffi.TerminalModes
import io.github.code_akram.or2.ffi.SessionException
import io.github.code_akram.or2.ffi.SessionInterface
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalCell
import io.github.code_akram.or2.ffi.TerminalFrame
import io.github.code_akram.or2.ffi.TerminalRow
import io.github.code_akram.or2.ffi.TerminalTransport
import io.github.code_akram.or2.ffi.Underline
import io.github.code_akram.or2.ffi.ViewportScroll
import org.junit.Assert.*
import org.junit.Test

class TerminalSessionTest {
    private val frame = TerminalFrame(1u, 1u, 1u, true,
        listOf(CellStyle(0xff0000u, 0u, null, Underline.NONE, false, false, false, false, false)),
        listOf(TerminalRow(0u, false, listOf(TerminalCell("A", CellWidth.NARROW, 0u)))),
        null, 0u, Scrollback(1u, 0u), TerminalModes(false, false))

    private class FakeSession(var frame: TerminalFrame?) : SessionInterface {
        var destroyed = false
        var calls = 0
        var inputError: SessionException? = null
        private fun touch() {
            calls++
            if (destroyed) throw IllegalStateException("Session object has already been destroyed")
        }
        override fun sendText(text: String) { touch(); inputError?.let { throw it } }
        override fun submitText(text: String) { touch(); inputError?.let { throw it } }
        override fun sendKey(input: KeyInput) { touch(); inputError?.let { throw it } }
        override fun resize(columns: UShort, rows: UShort) { touch() }
        override fun scroll(scroll: ViewportScroll) { touch() }
        override fun requestFullFrame() { touch() }
        override fun takeFrame(): TerminalFrame? { touch(); return frame.also { frame = null } }
        override fun state(): SessionState { touch(); return SessionState.Connected }
        override fun approveHostKey(fingerprint: String) { touch() }
        override fun rejectHostKey() { touch() }
        override fun disconnect() { touch() }
        override fun transport(): TerminalTransport { touch(); return TerminalTransport.SSH }
        override fun serverPid(): UInt? = null
        override fun roam() { touch() }
    }

    @Test fun destroyedInputStopsAllSubsequentCallsAndRetainsTheLastGrid() {
        val fake = FakeSession(frame)
        val access = TerminalSession().apply { bind(fake) }
        val grid = TerminalGrid().apply { apply(access.takeFrame()!!) }
        fake.destroyed = true
        assertFalse(access.call { sendText("late") })
        assertTrue(access.gone)
        assertNull(access.takeFrame())
        assertFalse(access.call { resize(40u, 10u) })
        assertFalse(access.call { scroll(ViewportScroll.Bottom) })
        assertEquals(2, fake.calls) // One frame take, one failed input; no calls after destruction.
        assertEquals("A", grid.rows.single().cells.single().text)
        assertEquals(0xff0000u, grid.rows.single().cells.single().style.foreground)
    }

    @Test fun destroyedVsyncTakeStopsInputAndRepeatedPulls() {
        val fake = FakeSession(frame).apply { destroyed = true }
        val access = TerminalSession().apply { bind(fake) }
        assertNull(access.takeFrame())
        assertTrue(access.gone)
        assertNull(access.takeFrame())
        assertFalse(access.call { sendText("late") })
        assertEquals(1, fake.calls)
    }

    @Test fun inputFollowsTheRouteAtCallTimeWhileFramesStayWithTheBoundHandle() {
        val old = FakeSession(frame)
        val next = FakeSession(null)
        var current: SessionInterface = old
        val access = TerminalSession().apply { bind(old, SessionRoute { current }) }
        assertTrue(access.call { sendText("a") })
        assertEquals(1, old.calls)
        // The terminal's handle is replaced (the swap) and the old one refuses input: the view, still
        // bound to the old handle, types into the new one.
        current = next
        old.inputError = SessionException.NotConnected()
        assertTrue(access.call { sendText("b") })
        assertEquals(1, next.calls)
        assertEquals(1, old.calls)
        assertTrue(access.callOwn { requestFullFrame() }) // This view's own frames: the bound handle.
        assertEquals(2, old.calls)
        assertEquals(frame, access.takeFrame()) // Frames are still the bound handle's.
        // The old handle destroyed ends the view's frames, never its input.
        old.destroyed = true
        assertNull(access.takeFrame())
        assertTrue(access.gone)
        assertTrue(access.call { sendText("c") })
        assertEquals(2, next.calls)
        // A destroyed route target is refused without ending anything more.
        next.destroyed = true
        assertFalse(access.call { sendText("d") })
    }

    @Test fun notConnectedIsTransientAndClosedStillAllowsTheFinalFrame() {
        val fake = FakeSession(frame)
        val access = TerminalSession().apply { bind(fake) }
        fake.inputError = SessionException.NotConnected()
        assertFalse(access.call { sendText("early") })
        assertFalse(access.gone)
        fake.inputError = null
        assertTrue(access.call { sendText("ready") })
        fake.inputError = SessionException.Closed()
        assertFalse(access.call { sendText("closed") })
        assertFalse(access.gone)
        assertEquals(frame, access.takeFrame())
    }
}
