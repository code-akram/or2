package io.github.code_akram.or2.app

import androidx.activity.compose.setContent
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertTextContains
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import io.github.code_akram.or2.MainActivity
import io.github.code_akram.or2.connection.ReconnectOffer
import io.github.code_akram.or2.connection.uiHost
import io.github.code_akram.or2.ui.Or2Theme
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test

/** The reconnect chip is a small non-modal control: tap to reconnect, the close glyph to dismiss. */
class ReconnectChipDeviceTest {
    @get:Rule val compose = createAndroidComposeRule<MainActivity>()

    private val calls = mutableListOf<String>()

    @Test
    fun tappingTheLabelReconnectsAndTheCloseGlyphDismisses() {
        val offer = ReconnectOffer(listOf(uiHost(1, "Alpha"), uiHost(2, "Beta")), prompts = 2)
        compose.runOnUiThread {
            compose.activity.setContent {
                Or2Theme { ReconnectChip(offer, reconnect = { calls += "reconnect" }, dismiss = { calls += "dismiss" }) }
            }
        }
        compose.onNodeWithTag("reconnect-chip").assertIsDisplayed()
        compose.onNodeWithTag("reconnect-confirm").assertTextContains("Reconnect 2 hosts", substring = true)
        compose.onNodeWithTag("reconnect-confirm").assertTextContains("2 fingerprints", substring = true)
        compose.onNodeWithTag("reconnect-confirm").performClick()
        compose.onNodeWithTag("reconnect-dismiss").performClick()
        compose.runOnIdle { assertEquals(listOf("reconnect", "dismiss"), calls) }
    }
}
