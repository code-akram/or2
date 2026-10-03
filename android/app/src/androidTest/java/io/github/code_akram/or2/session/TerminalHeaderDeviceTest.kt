package io.github.code_akram.or2.session

import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertTextEquals
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.test.swipeDown
import io.github.code_akram.or2.MainActivity
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.LinkHealth
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.terminal.Transport
import io.github.code_akram.or2.ui.Or2Theme
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

/** The terminal card's header: the disc pair, the centred title, the handle, and the notice strip under it. */
class TerminalHeaderDeviceTest {
    @get:Rule val compose = createAndroidComposeRule<MainActivity>()

    private var host by mutableStateOf("workstation")
    private var target by mutableStateOf("tmux main")
    private var state by mutableStateOf<SessionState>(SessionState.Connected)
    private var health by mutableStateOf<LinkHealth?>(null)
    private var minimised = 0
    private var closed = 0

    private fun show() = compose.runOnUiThread {
        compose.activity.setContent {
            Or2Theme {
                TerminalCard(
                    host, target, Transport.MOSH, state, minimise = { minimised++ }, openSwitcher = {}, endSession = { closed++ },
                    linkHealth = health,
                ) { Box(Modifier.weight(1f).fillMaxWidth().testTag("terminal-body")) }
            }
        }
    }

    private fun bounds(tag: String, unmerged: Boolean = false): Rect =
        compose.onNodeWithTag(tag, useUnmergedTree = unmerged).fetchSemanticsNode().boundsInRoot

    private fun assertCentredOnTheCard() {
        val card = bounds("terminal-card")
        val title = bounds("terminal-title", unmerged = true)
        assertEquals("title $title on card $card", card.center.x, title.center.x, 2f)
    }

    private fun assertClearOfTheSides() {
        val title = bounds("terminal-title", unmerged = true)
        assertTrue("title $title overlaps the discs", title.left >= bounds("terminal-panes").right)
        assertTrue("title $title overlaps the pill", title.right <= bounds("terminal-transport", unmerged = true).left)
        if (health != null) assertTrue("title $title overlaps the stale label", title.right <= bounds("terminal-link").left)
    }

    @Test
    fun theTitleIsCentredOnTheCardAndReadsHostThenTarget() {
        show()
        compose.onNodeWithTag("terminal-title", useUnmergedTree = true).assertTextEquals("workstation · tmux main")
        assertCentredOnTheCard()
        assertClearOfTheSides()
        // The handle sits centred over the title, inside the header.
        val handle = bounds("terminal-handle")
        val header = bounds("terminal-header")
        assertEquals(bounds("terminal-card").center.x, handle.center.x, 2f)
        assertTrue("handle $handle in header $header", handle.top >= header.top && handle.bottom <= bounds("terminal-title", unmerged = true).top)
    }

    @Test
    fun aLongTitleEllipsizesStillCentredAndClearOfBothSides() {
        show()
        compose.runOnUiThread {
            host = "build-box-staging-eu-west-with-a-very-long-name"
            target = "herdr personal w1:p2 and a pane title that goes on"
        }
        compose.waitForIdle()
        assertCentredOnTheCard()
        assertClearOfTheSides()
        // A stale link widens the right side: the title gives way on both sides and stays centred.
        compose.runOnUiThread { health = LinkHealth(12_300uL) }
        compose.onNodeWithTag("terminal-link").assertIsDisplayed()
        assertCentredOnTheCard()
        assertClearOfTheSides()
    }

    @Test
    fun theNoticeStripTakesLayoutSpaceAndClosedCarriesClose() {
        show()
        compose.onNodeWithTag("terminal-notice").assertDoesNotExist()
        val connected = bounds("terminal-body")
        compose.runOnUiThread { state = SessionState.Connecting }
        compose.onNodeWithTag("terminal-status", useUnmergedTree = true).assertTextEquals("Connecting…")
        compose.onNodeWithTag("terminal-close").assertDoesNotExist()
        // The strip sits between the header and the grid: it pushes the grid down, it never covers it.
        val strip = bounds("terminal-notice")
        assertTrue("strip $strip under the header", strip.top >= bounds("terminal-header").bottom - 1)
        assertTrue("strip $strip over the body", strip.bottom <= bounds("terminal-body").top + 1)
        assertTrue(bounds("terminal-body").top > connected.top)
        compose.runOnUiThread { state = SessionState.Closed(CloseReason.Disconnected) }
        compose.onNodeWithTag("terminal-status", useUnmergedTree = true).assertTextEquals("Disconnected")
        compose.onNodeWithTag("terminal-close").assertIsDisplayed().performClick()
        compose.runOnIdle { assertEquals(1, closed) }
    }

    @Test
    fun aDragDownOnTheHeaderMinimises() {
        show()
        compose.onNodeWithTag("terminal-header").performTouchInput { swipeDown(startY = top + 4f, endY = top + 200 * density) }
        compose.runOnIdle { assertEquals(1, minimised) }
    }
}
