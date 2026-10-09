package io.github.code_akram.or2.session

import io.github.code_akram.or2.ffi.HistoryText
import io.github.code_akram.or2.ffi.HostException
import io.github.code_akram.or2.ffi.TerminalTarget
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The history sheet's model: what it shows and copies of a `read_history` answer, and which terminals offer it. */
class HistorySheetTest {
    @Test
    fun controlCharactersOtherThanTabAreRemovedAndLineBreaksKept() {
        assertEquals("a\tb\nbell gone\n[1m bold", sanitizeHistory("a\tb\r\nbell\u0007 gone\n\u001b[1m bold"))
        // DEL and the C1 controls go too; other text, wide and combining characters included, stays.
        assertEquals("x y é 漢", sanitizeHistory("x\u007f y\u0085 é 漢\u009b"))
    }

    @Test
    fun trailingBlanksOfEachLineAndTheEmptyLinesAtTheEndAreDropped() {
        assertEquals("prompt $ ls\nfile", sanitizeHistory("prompt $ ls   \nfile\t\n\n   \n\n"))
        // Blank lines in the middle are part of the history.
        assertEquals("a\n\nb", sanitizeHistory("a\n\nb\n"))
        assertEquals("", sanitizeHistory("\n \n"))
    }

    @Test
    fun theNoteCountsTheLinesShownAndSaysWhenOlderOnesWereLeftOut() {
        val loaded = historyLoaded(HistoryText("one\ntwo\nthree\n\n", false))
        assertEquals(HistoryLoad.Loaded("one\ntwo\nthree", 3, false), loaded)
        assertEquals("3 lines", historyNote(loaded))
        val cut = historyLoaded(HistoryText("newest", true))
        assertEquals("1 line · older lines not shown", historyNote(cut))
        assertEquals("No lines", historyNote(historyLoaded(HistoryText("", false))))
    }

    @Test
    fun copyAllCopiesTheTextAsShownAndNothingWithoutIt() {
        val loaded = historyLoaded(HistoryText("\u001b]0;title\u0007ok  \n", true))
        assertEquals("]0;titleok", historyCopyText(loaded))
        assertNull(historyCopyText(HistoryLoad.Loading))
        assertNull(historyCopyText(HistoryLoad.Failed("no")))
        assertNull(historyCopyText(historyLoaded(HistoryText("\n\n", false))))
    }

    @Test
    fun onlyTmuxAndHerdrTerminalsOfferHistory() {
        assertTrue(hasHistory(TerminalTarget.Tmux("work")))
        assertTrue(hasHistory(TerminalTarget.Herdr(null, null)))
        assertTrue(hasHistory(TerminalTarget.Herdr("work", "w1:p2")))
        assertFalse(hasHistory(TerminalTarget.Shell))
        assertFalse(hasHistory(TerminalTarget.ShellIn("/srv/app")))
    }

    @Test
    fun aFailedReadSaysWhatWentWrong() {
        assertEquals("This herdr pane no longer exists.", historyErrorMessage(HostException.PaneNotFound()))
        assertEquals(
            "The host could not read the history. Retry, or reconnect.",
            historyErrorMessage(HostException.CommandFailed("tmux failed: no server")),
        )
        assertEquals("tmux is not installed on the host.", historyErrorMessage(HostException.NotInstalled("tmux")))
    }
}
