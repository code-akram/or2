package io.github.code_akram.or2.session

import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.assert
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertTextEquals
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithTag
import io.github.code_akram.or2.MainActivity
import io.github.code_akram.or2.ffi.LinkHealth
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.terminal.Transport
import io.github.code_akram.or2.ui.Or2Theme
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

/** The terminal header's transport badge and link-health text, without any connection. */
class TransportChromeDeviceTest {
    @get:Rule val compose = createAndroidComposeRule<MainActivity>()

    private var transport by mutableStateOf(Transport.MOSH)
    private var health by mutableStateOf<LinkHealth?>(null)

    private fun show() = compose.runOnUiThread {
        compose.activity.setContent {
            Or2Theme {
                TerminalCard(
                    "workstation: tmux main", transport, SessionState.Connected, minimise = {}, openSwitcher = {}, endSession = {},
                    linkHealth = health,
                ) { Box(Modifier.weight(1f).fillMaxWidth().testTag("terminal-body")) }
            }
        }
    }

    private fun assertBadgeGreyed(greyed: Boolean) {
        val badge = compose.onNodeWithTag("terminal-transport", useUnmergedTree = true)
        if (greyed) badge.assert(SemanticsMatcher.expectValue(SemanticsProperties.StateDescription, STALE_BADGE_DESCRIPTION))
        else badge.assert(SemanticsMatcher.keyNotDefined(SemanticsProperties.StateDescription))
    }

    @Test
    fun theBadgeShowsTheRealTransport() {
        show()
        compose.onNodeWithTag("terminal-transport", useUnmergedTree = true).assertTextEquals("Mosh")
        compose.runOnUiThread { transport = Transport.SSH }
        compose.onNodeWithTag("terminal-transport", useUnmergedTree = true).assertTextEquals("SSH")
    }

    @Test
    fun aQuietMoshLinkSaysHowLongAgoOnlyPastFiveSeconds() {
        show()
        compose.onNodeWithTag("terminal-link").assertDoesNotExist()
        assertBadgeGreyed(false)
        compose.runOnUiThread { health = LinkHealth(300uL, 300uL) }
        compose.onNodeWithTag("terminal-link").assertDoesNotExist()
        assertBadgeGreyed(false)
        compose.runOnUiThread { health = LinkHealth(5000uL, 9000uL) }
        compose.onNodeWithTag("terminal-link").assertDoesNotExist() // Exactly five seconds is not stale yet.
        assertBadgeGreyed(false)
        compose.runOnUiThread { health = LinkHealth(12_300uL, 12_300uL) }
        compose.onNodeWithTag("terminal-link").assertIsDisplayed().assertTextEquals("Last heard 12 s ago")
        compose.onNodeWithTag("terminal-transport", useUnmergedTree = true).assertTextEquals("Mosh") // Greyed, not renamed.
        assertBadgeGreyed(true)
        compose.runOnUiThread { health = LinkHealth(400uL, 400uL) }
        compose.onNodeWithTag("terminal-link").assertDoesNotExist() // Recovered.
        assertBadgeGreyed(false)
    }

    @Test
    fun theLinkLineSitsInTheHeaderNeverOverTheTerminalAndNeverResizesIt() {
        show()
        val body = { compose.onNodeWithTag("terminal-body").fetchSemanticsNode().boundsInRoot }
        val plain = body()
        compose.runOnUiThread { health = LinkHealth(12_300uL, 12_300uL) }
        val link = compose.onNodeWithTag("terminal-link").assertIsDisplayed().fetchSemanticsNode().boundsInRoot
        // It takes the header's own space: no row of the terminal is covered.
        assertTrue("the link line overlaps the terminal", link.bottom <= body().top)
        // And a flapping link must not make the grid (and the remote) resize each time.
        assertEquals(plain, body())
        compose.runOnUiThread { health = LinkHealth(400uL, 400uL) }
        compose.onNodeWithTag("terminal-link").assertDoesNotExist()
        assertEquals(plain, body())
    }

    @Test
    fun noNoteIsDrawnUnderTheHeader() {
        show()
        compose.runOnUiThread { transport = Transport.SSH }
        compose.onNodeWithTag("terminal-note").assertDoesNotExist()
    }
}
