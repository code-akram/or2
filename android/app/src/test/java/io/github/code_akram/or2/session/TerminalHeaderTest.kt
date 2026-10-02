package io.github.code_akram.or2.session

import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.PublicKeyInfo
import io.github.code_akram.or2.ffi.SessionState
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
    fun theTitleReadsHostThenTarget() {
        assertEquals("workstation · tmux main", headerTitleText("workstation", "tmux main"))
    }

    @Test
    fun connectingIsAMutedBusyNoticeAndClosedAWarningWithClose() {
        assertNull(terminalNotice(SessionState.Connected))
        listOf(
            SessionState.Connecting, SessionState.Authenticating,
            SessionState.AwaitingHostKeyDecision(PublicKeyInfo("ssh-ed25519", "k", "SHA256:x", ""), emptyList()),
        ).forEach {
            val notice = terminalNotice(it)!!
            assertEquals(NoticeTone.Info, notice.tone)
            assertTrue(notice.busy)
            assertTrue(!notice.closable)
            assertEquals(sessionMessage(it), notice.text)
        }
        val closed = terminalNotice(SessionState.Closed(CloseReason.Disconnected))!!
        assertEquals(TerminalNotice("Disconnected", NoticeTone.Warning, busy = false, closable = true), closed)
    }

    @Test
    fun theStripIsTintedBySeverity() {
        assertEquals(Or2Colors.TextMuted, noticeColor(NoticeTone.Info))
        assertEquals(Or2Colors.Attention, noticeColor(NoticeTone.Warning))
        assertEquals(Or2Icons.Info, noticeIcon(NoticeTone.Info))
        assertEquals(Or2Icons.Warning, noticeIcon(NoticeTone.Warning))
    }
}
