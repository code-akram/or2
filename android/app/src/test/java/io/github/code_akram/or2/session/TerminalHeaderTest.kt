package io.github.code_akram.or2.session

import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.SessionFailure
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.paste.UploadState
import io.github.code_akram.or2.paste.uploadNotice
import io.github.code_akram.or2.ui.NoticeTone
import io.github.code_akram.or2.ui.Or2Colors
import io.github.code_akram.or2.ui.Or2Icons
import io.github.code_akram.or2.ui.noticeColor
import io.github.code_akram.or2.ui.noticeIcon
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The header's centring arithmetic and the notice strip's variants. */
class TerminalHeaderTest {
    @Test
    fun theTitleSlotIsSymmetricAboutTheCentreWhateverTheSides() {
        // 1000 wide, discs 60, pill 50, gap 8: the wider side (60 + 8) is taken off both ends.
        assertEquals(1000 - 2 * 68, centredSlotWidth(1000, 60, 50, 8))
        // A stale label makes the right side the wider one: the slot shrinks on both sides.
        assertEquals(1000 - 2 * 208, centredSlotWidth(1000, 60, 200, 8))
        // Sides wider than half the row leave no slot, never a negative one.
        assertEquals(0, centredSlotWidth(300, 60, 200, 8))
    }

    @Test
    fun theTitleIsCentredOnTheWholeWidthAndNeverMeetsEitherSide() {
        val width = 1440
        listOf(Triple(91, 140, 300), Triple(91, 430, 1200), Triple(91, 140, 40)).forEach { (leading, trailing, wanted) ->
            val slot = centredSlotWidth(width, leading, trailing, 28)
            val title = minOf(wanted, slot) // A Text that overflows takes the whole slot and ellipsizes.
            val x = centredX(width, title)
            // Centred on the row, not on the space between the sides ...
            assertTrue("centre ${x + title / 2}", kotlin.math.abs(width / 2 - (x + title / 2)) <= 1)
            // ... and clear of both, with the gap.
            assertTrue("x $x after leading $leading", x >= leading + 28)
            assertTrue("end ${x + title} before trailing $trailing", x + title <= width - trailing - 28)
        }
    }

    @Test
    fun onlyAClosedTerminalHasANoticeAWarningWithClose() {
        // The card shows once the terminal has connected: nothing but Closed needs explaining under its header.
        assertNull(terminalNotice(SessionState.Connected))
        assertNull(terminalNotice(SessionState.Connecting))
        val closed = terminalNotice(SessionState.Closed(CloseReason.Disconnected))!!
        assertEquals(TerminalNotice("Disconnected", NoticeTone.Warning, busy = false, action = "Close"), closed)
        val failed = SessionState.Closed(CloseReason.Failed(SessionFailure.ConnectionLost("diagnostic")))
        assertEquals(sessionMessage(failed), terminalNotice(failed)!!.text)
    }

    @Test
    fun anUploadTakesTheSameStripWithItsOwnAction() {
        assertNull(uploadNotice(UploadState.Idle))
        assertEquals(TerminalNotice("Uploading image…", NoticeTone.Info, busy = true, action = "Cancel"), uploadNotice(UploadState.Uploading()))
        assertEquals(TerminalNotice("No room", NoticeTone.Warning, busy = false, action = "Dismiss"), uploadNotice(UploadState.Failed("No room")))
    }

    @Test
    fun theStripIsTintedBySeverity() {
        assertEquals(Or2Colors.TextMuted, noticeColor(NoticeTone.Info))
        assertEquals(Or2Colors.Attention, noticeColor(NoticeTone.Warning))
        assertEquals(Or2Icons.Info, noticeIcon(NoticeTone.Info))
        assertEquals(Or2Icons.Warning, noticeIcon(NoticeTone.Warning))
    }
}
