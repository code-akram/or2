package io.github.code_akram.or2.terminal

import org.junit.Assert.assertEquals
import org.junit.Test

/** A single tap on the terminal (docs/ui.md): selection, then link, then click, then keyboard. */
class TerminalTapTest {
    @Test fun aSelectionIsClearedFirstAndNeverFollowedThroughALinkOrClicked() {
        for (link in listOf(false, true)) for (mouse in listOf(false, true)) {
            assertEquals(TapAction.CLEAR_SELECTION, tapAction(selecting = true, link = link, mouseTracking = mouse))
        }
    }

    @Test fun aLinkOpensEvenWhileTheProgramTracksTheMouse() {
        assertEquals(TapAction.OPEN_LINK, tapAction(selecting = false, link = true, mouseTracking = true))
        assertEquals(TapAction.OPEN_LINK, tapAction(selecting = false, link = true, mouseTracking = false))
    }

    @Test fun withMouseTrackingATapIsAClickAndDoesNotShowTheKeyboard() {
        assertEquals(TapAction.CLICK, tapAction(selecting = false, link = false, mouseTracking = true))
    }

    @Test fun withoutMouseTrackingATapShowsTheKeyboardAsBefore() {
        assertEquals(TapAction.SHOW_KEYBOARD, tapAction(selecting = false, link = false, mouseTracking = false))
    }
}
