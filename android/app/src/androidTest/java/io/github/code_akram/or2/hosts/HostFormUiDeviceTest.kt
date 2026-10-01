package io.github.code_akram.or2.hosts

import androidx.activity.compose.setContent
import androidx.compose.material3.MaterialTheme
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollTo
import androidx.compose.ui.test.performTextClearance
import androidx.compose.ui.test.performTextInput
import io.github.code_akram.or2.MainActivity
import io.github.code_akram.or2.connection.uiHost
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.HostEndpoint
import io.github.code_akram.or2.data.KeyRecord
import io.github.code_akram.or2.inbox.LinkStatus
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Rule
import org.junit.Test

/** The host form's ordered address list, port per address and inbox switch, without any storage. */
class HostFormUiDeviceTest {
    @get:Rule val compose = createAndroidComposeRule<MainActivity>()

    private val key = KeyRecord("fixture-key", "Fixture key", "test", "public", "fingerprint", "", byteArrayOf(), byteArrayOf())

    private fun show(hosts: List<Host>, save: (Host, Host?) -> Unit, link: (Host) -> LinkStatus = { LinkStatus.NOT_CONNECTED }) =
        compose.runOnUiThread {
            compose.activity.setContent { MaterialTheme { HostsScreen(hosts, listOf(key), false, save, {}, {}, link = link) } }
        }

    private fun fill(index: Int, hostname: String, port: String? = null) {
        compose.onNodeWithTag("address-hostname:$index").performScrollTo().performTextInput(hostname)
        if (port != null) compose.onNodeWithTag("address-port:$index").performScrollTo().apply { performTextClearance(); performTextInput(port) }
    }

    @Test
    fun addressesCanBeAddedReorderedRemovedAndEachKeepsItsPort() {
        var saved: Host? = null
        show(emptyList(), { host, _ -> saved = host })
        compose.onNodeWithText("Add host").performClick()
        compose.onNodeWithText("Label").performScrollTo().performTextInput("Multi")
        compose.onNodeWithText("Username").performScrollTo().performTextInput("fixture-user")
        fill(0, "first.invalid")
        compose.onNodeWithTag("address-add").performScrollTo().performClick()
        fill(1, "second.invalid", "2222")
        compose.onNodeWithTag("address-add").performScrollTo().performClick()
        fill(2, "third.invalid", "2022")
        compose.onNodeWithTag("address-up:0").assertIsNotEnabled() // The first cannot move up ...
        compose.onNodeWithTag("address-down:2").assertIsNotEnabled() // ... nor the last down.
        compose.onNodeWithTag("address-up:2").performScrollTo().performClick() // third <-> second
        compose.onNodeWithTag("address-remove:0").performScrollTo().performClick() // drops first.invalid
        compose.onNodeWithText("Save").performScrollTo().assertIsEnabled().performClick()
        compose.runOnIdle {
            assertEquals(listOf(HostEndpoint("third.invalid", 2022), HostEndpoint("second.invalid", 2222)), saved!!.addresses)
            assertEquals("fixture-key", saved!!.keyId) // The only key is preselected.
            assertEquals(true, saved!!.showInInbox)
        }
    }

    @Test
    fun anInvalidPortInAnyAddressBlocksSavingAndTheLastAddressCannotBeRemoved() {
        show(emptyList(), { _, _ -> })
        compose.onNodeWithText("Add host").performClick()
        compose.onNodeWithText("Label").performScrollTo().performTextInput("Multi")
        compose.onNodeWithText("Username").performScrollTo().performTextInput("fixture-user")
        fill(0, "first.invalid")
        compose.onNodeWithTag("address-remove:0").performScrollTo().assertIsNotEnabled()
        compose.onNodeWithText("Save").performScrollTo().assertIsEnabled()
        compose.onNodeWithTag("address-add").performScrollTo().performClick()
        fill(1, "second.invalid", "70000")
        compose.onNodeWithText("Save").performScrollTo().assertIsNotEnabled()
        compose.onNodeWithTag("address-port:1").performScrollTo().apply { performTextClearance(); performTextInput("22") }
        compose.onNodeWithText("Save").performScrollTo().assertIsEnabled()
    }

    @Test
    fun theInboxSwitchAndEditingAnExistingAddressListRoundTrip() {
        var saved: Host? = null
        var before: Host? = null
        val existing = uiHost(3, "Existing", addresses = listOf(HostEndpoint("a.invalid", 22), HostEndpoint("b.invalid", 2222)))
        show(listOf(existing), { host, previous -> saved = host; before = previous })
        compose.onNodeWithText("Edit").performClick()
        compose.onNodeWithText("Changing any address or port clears previous host-key trust.").performScrollTo()
        compose.onNodeWithTag("address-down:0").performScrollTo().performClick()
        compose.onNodeWithTag("host-inbox").performScrollTo().performClick()
        compose.onNodeWithText("Save").performScrollTo().performClick()
        compose.runOnIdle {
            assertEquals(existing, before)
            assertEquals(listOf(HostEndpoint("b.invalid", 2222), HostEndpoint("a.invalid", 22)), saved!!.addresses)
            assertFalse(saved!!.showInInbox)
            assertEquals(3L, saved!!.id)
        }
    }

    @Test
    fun hostCardsShowEveryAddressAndOnlyOfferConnectWhenNotConnected() {
        val existing = uiHost(3, "Existing", addresses = listOf(HostEndpoint("a.invalid", 22), HostEndpoint("b.invalid", 2222)))
        show(listOf(existing), { _, _ -> }, link = { LinkStatus.CONNECTED })
        compose.onNodeWithText("fixture-user@a.invalid:22").assertExists()
        compose.onNodeWithText("Also: b.invalid:2222").assertExists()
        compose.onNodeWithText("Connected").assertExists()
        compose.onNodeWithText("Connect").assertIsNotEnabled()
        show(listOf(existing), { _, _ -> })
        compose.onNodeWithText("Connect").assertIsEnabled()
    }
}
