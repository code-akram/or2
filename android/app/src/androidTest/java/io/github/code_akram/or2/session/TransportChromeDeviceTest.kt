package io.github.code_akram.or2.session

import androidx.activity.compose.setContent
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertTextEquals
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithTag
import io.github.code_akram.or2.MainActivity
import io.github.code_akram.or2.ffi.LinkHealth
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.terminal.Transport
import io.github.code_akram.or2.ui.Or2Theme
import org.junit.Rule
import org.junit.Test

/** The terminal header's transport badge, link-health text and fallback note, without any connection. */
class TransportChromeDeviceTest {
    @get:Rule val compose = createAndroidComposeRule<MainActivity>()

    private var transport by mutableStateOf(Transport.MOSH)
    private var health by mutableStateOf<LinkHealth?>(null)
    private var note by mutableStateOf<String?>(null)

    private fun show() = compose.runOnUiThread {
        compose.activity.setContent {
            Or2Theme {
                TerminalCard(
                    "workstation: tmux main", transport, SessionState.Connected, minimise = {}, openSwitcher = {}, endSession = {},
                    linkHealth = health, note = note,
                ) {}
            }
        }
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
        compose.runOnUiThread { health = LinkHealth(300uL, 300uL) }
        compose.onNodeWithTag("terminal-link").assertDoesNotExist()
        compose.runOnUiThread { health = LinkHealth(5000uL, 9000uL) }
        compose.onNodeWithTag("terminal-link").assertDoesNotExist() // Exactly five seconds is not stale yet.
        compose.runOnUiThread { health = LinkHealth(12_300uL, 12_300uL) }
        compose.onNodeWithTag("terminal-link").assertIsDisplayed().assertTextEquals("Last heard 12 s ago")
        compose.onNodeWithTag("terminal-transport", useUnmergedTree = true).assertTextEquals("Mosh") // Greyed, not renamed.
        compose.runOnUiThread { health = LinkHealth(400uL, 400uL) }
        compose.onNodeWithTag("terminal-link").assertDoesNotExist() // Recovered.
    }

    @Test
    fun theFallbackNoteIsShownInMutedText() {
        show()
        compose.onNodeWithTag("terminal-note").assertDoesNotExist()
        compose.runOnUiThread { transport = Transport.SSH; note = "Mosh could not reach the host over UDP. Using SSH for this connection." }
        compose.onNodeWithTag("terminal-note").assertIsDisplayed()
            .assertTextEquals("Mosh could not reach the host over UDP. Using SSH for this connection.")
    }
}
