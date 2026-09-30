package io.github.code_akram.or2.session

import androidx.activity.compose.setContent
import androidx.compose.material3.MaterialTheme
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import io.github.code_akram.or2.MainActivity
import io.github.code_akram.or2.ffi.PublicKeyInfo
import io.github.code_akram.or2.ffi.SessionState
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test

/** Render trust states without connecting to any host or modifying the production database. */
class EntryUiDeviceTest {
    @get:Rule val compose = createAndroidComposeRule<MainActivity>()

    @Test
    fun firstUseTrustIsExplicitAndRejectWorks() {
        var decision = ""
        val prompt = SessionState.AwaitingHostKeyDecision(PublicKeyInfo("test", "public", "presented-fingerprint", ""), emptyList())
        compose.runOnUiThread {
            compose.activity.setContent { MaterialTheme { HostTrustDialog(prompt, false, { decision = "approve" }, { decision = "reject" }) } }
        }
        compose.onNodeWithText("Trust this host key?").assertIsDisplayed()
        compose.onNodeWithText("presented-fingerprint").assertIsDisplayed()
        assertEquals("", decision)
        compose.onNodeWithText("Reject").performClick()
        assertEquals("reject", decision)
    }

    @Test
    fun changedTrustShowsAllPreviousFingerprintsAndReplacementLabel() {
        var decision = ""
        val old = listOf(PublicKeyInfo("old-a", "a", "previous-a", ""), PublicKeyInfo("old-b", "b", "previous-b", ""))
        val prompt = SessionState.AwaitingHostKeyDecision(PublicKeyInfo("new", "c", "presented-fingerprint", ""), old)
        compose.runOnUiThread {
            compose.activity.setContent { MaterialTheme { HostTrustDialog(prompt, false, { decision = "approve" }, { decision = "reject" }) } }
        }
        compose.onNodeWithText("WARNING: HOST KEY CHANGED").assertIsDisplayed()
        compose.onNodeWithText("old-a\nprevious-a").assertIsDisplayed()
        compose.onNodeWithText("old-b\nprevious-b").assertIsDisplayed()
        compose.onNodeWithText("presented-fingerprint").assertIsDisplayed()
        compose.onNodeWithText("Replace trust and connect").performClick()
        assertEquals("approve", decision)
    }
}
