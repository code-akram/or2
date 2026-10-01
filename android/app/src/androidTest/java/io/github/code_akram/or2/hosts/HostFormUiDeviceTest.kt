package io.github.code_akram.or2.hosts

import androidx.activity.compose.setContent
import androidx.compose.runtime.key
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.assert
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.assertIsNotSelected
import androidx.compose.ui.test.assertIsSelected
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithContentDescription
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
import io.github.code_akram.or2.data.HostRecord
import io.github.code_akram.or2.data.KeyRecord
import io.github.code_akram.or2.ui.Or2Theme
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Rule
import org.junit.Test

/** The host form: ordered address list with a port per address, key choice and inbox toggle, without any storage. */
class HostFormUiDeviceTest {
    @get:Rule val compose = createAndroidComposeRule<MainActivity>()

    private var generations = 0

    private val key = KeyRecord("fixture-key", "Fixture key", "test", "public", "SHA256:fingerprint", "", byteArrayOf(), byteArrayOf())

    private fun show(previous: Host?, keys: List<KeyRecord> = listOf(key), save: (Host) -> Unit = {}, close: () -> Unit = {}) =
        compose.runOnUiThread {
            val generation = ++generations
            // A new key per call: remember state must not leak from the previous show().
            compose.activity.setContent { key(generation) { Or2Theme { HostFormScreen(previous, keys, false, save, close) } } }
        }

    private fun keyChoice(key: KeyRecord) = compose.onNodeWithTag("host-key:${key.id}")
        .assert(SemanticsMatcher.expectValue(SemanticsProperties.Role, Role.RadioButton))

    private fun fill(index: Int, hostname: String, port: String? = null) {
        compose.onNodeWithTag("address-hostname:$index").performScrollTo().performTextInput(hostname)
        if (port != null) compose.onNodeWithTag("address-port:$index").performScrollTo().apply { performTextClearance(); performTextInput(port) }
    }

    private fun fillHostFields() {
        compose.onNodeWithTag("host-label").performScrollTo().performTextInput("Fixture")
        fill(0, "fixture.invalid")
        compose.onNodeWithTag("host-username").performScrollTo().performTextInput("fixture-user")
    }

    @Test
    fun addressesCanBeAddedReorderedRemovedAndEachKeepsItsPort() {
        var saved: Host? = null
        show(null, save = { saved = it })
        compose.onNodeWithTag("host-label").performScrollTo().performTextInput("Multi")
        compose.onNodeWithTag("host-username").performScrollTo().performTextInput("fixture-user")
        fill(0, "first.invalid")
        compose.onNodeWithTag("address-add").performScrollTo().performClick()
        fill(1, "second.invalid", "2222")
        compose.onNodeWithTag("address-add").performScrollTo().performClick()
        fill(2, "third.invalid", "2022")
        compose.onNodeWithTag("address-up:0").performScrollTo().assertIsNotEnabled() // The first cannot move up ...
        compose.onNodeWithTag("address-down:2").performScrollTo().assertIsNotEnabled() // ... nor the last down.
        compose.onNodeWithTag("address-up:2").performScrollTo().performClick() // third <-> second
        compose.onNodeWithTag("address-remove:0").performScrollTo().performClick() // drops first.invalid
        compose.onNodeWithTag("host-form-primary").performScrollTo().assertIsEnabled().performClick()
        compose.runOnIdle {
            assertEquals(listOf(HostEndpoint("third.invalid", 2022), HostEndpoint("second.invalid", 2222)), saved!!.addresses)
            assertEquals("fixture-key", saved!!.keyId) // The only key is preselected.
            assertEquals(true, saved!!.showInInbox)
        }
    }

    @Test
    fun anInvalidPortInAnyAddressBlocksSavingAndTheLastAddressCannotBeRemoved() {
        show(null)
        compose.onNodeWithTag("host-label").performScrollTo().performTextInput("Multi")
        compose.onNodeWithTag("host-username").performScrollTo().performTextInput("fixture-user")
        fill(0, "first.invalid")
        compose.onNodeWithTag("address-remove:0").performScrollTo().assertIsNotEnabled()
        compose.onNodeWithTag("host-form-primary").performScrollTo().assertIsEnabled()
        compose.onNodeWithTag("address-add").performScrollTo().performClick()
        fill(1, "second.invalid", "70000")
        compose.onNodeWithTag("host-form-primary").performScrollTo().assertIsNotEnabled()
        compose.onNodeWithTag("host-form-save").assertIsNotEnabled() // The top-bar check mirrors the button.
        compose.onNodeWithTag("address-port:1").performScrollTo().apply { performTextClearance(); performTextInput("22") }
        compose.onNodeWithTag("host-form-primary").performScrollTo().assertIsEnabled()
        compose.onNodeWithTag("host-form-save").assertIsEnabled()
    }

    @Test
    fun theInboxSwitchAndEditingAnExistingAddressListRoundTrip() {
        var saved: Host? = null
        val existing = uiHost(3, "Existing", addresses = listOf(HostEndpoint("a.invalid", 22), HostEndpoint("b.invalid", 2222)))
        show(existing, save = { saved = it })
        compose.onNodeWithText("Edit Connection").assertIsDisplayed()
        compose.onNodeWithText("Changing any address or port clears previous host-key trust.").performScrollTo().assertIsDisplayed()
        compose.onNodeWithTag("address-down:0").performScrollTo().performClick()
        compose.onNodeWithTag("host-inbox").performScrollTo().performClick()
        compose.onNodeWithTag("host-form-save").performClick() // The top-bar check saves too.
        compose.runOnIdle {
            assertEquals(listOf(HostEndpoint("b.invalid", 2222), HostEndpoint("a.invalid", 22)), saved!!.addresses)
            assertFalse(saved!!.showInInbox)
            assertEquals(3L, saved!!.id)
        }
    }

    @Test
    fun newHostPreselectsTheOnlyKeyButEditingDoesNotChangeAnEmptyReference() {
        var saved: Host? = null
        show(null, save = { saved = it })
        keyChoice(key).performScrollTo().assertIsSelected()
        compose.onNodeWithText("Choose a key").assertDoesNotExist()
        fillHostFields()
        compose.onNodeWithTag("host-form-primary").performScrollTo().assertIsEnabled().performClick()
        compose.runOnIdle { assertEquals(key.id, saved!!.keyId) }
        // An existing host without a key stays without one until the user chooses.
        show(Host(HostRecord(7, "Existing fixture", "fixture-user", null), listOf(HostEndpoint("fixture.invalid", 22))))
        keyChoice(key).performScrollTo().assertIsNotSelected()
        compose.onNodeWithText("Choose a key").performScrollTo().assertIsDisplayed()
        compose.onNodeWithTag("host-form-primary").performScrollTo().assertIsNotEnabled()
    }

    @Test
    fun multipleKeysNeedAnExplicitChoiceAndExposeTheSelectedRadioState() {
        var saved: Host? = null
        // Duplicate labels must not make the test target a different choice.
        val second = key.copy(id = "second-key")
        show(null, listOf(key, second), save = { saved = it })
        keyChoice(key).performScrollTo().assertIsNotSelected()
        keyChoice(second).performScrollTo().assertIsNotSelected()
        fillHostFields()
        compose.onNodeWithText("Choose a key").performScrollTo().assertIsDisplayed()
        compose.onNodeWithTag("host-form-primary").performScrollTo().assertIsNotEnabled()
        keyChoice(second).performScrollTo().performClick().assertIsSelected()
        keyChoice(key).assertIsNotSelected()
        compose.onNodeWithText("Choose a key").assertDoesNotExist()
        compose.onNodeWithTag("host-form-primary").performScrollTo().assertIsEnabled().performClick()
        compose.runOnIdle { assertEquals(second.id, saved!!.keyId) }
    }

    @Test
    fun withoutKeysTheFormPointsToTheKeysScreen() {
        var keysOpened = false
        compose.runOnUiThread {
            compose.activity.setContent { Or2Theme { HostFormScreen(null, emptyList(), false, {}, {}, openKeys = { keysOpened = true }) } }
        }
        compose.onNodeWithText("Generate or import a key on the Keys screen first.").performScrollTo().assertIsDisplayed()
        compose.onNodeWithTag("host-add-key").performScrollTo().performClick()
        compose.runOnIdle { assertEquals(true, keysOpened) }
    }

    @Test
    fun closingTheFormCallsBackWithoutSaving() {
        var closed = 0
        var saved = 0
        show(null, save = { saved++ }, close = { closed++ })
        compose.onNodeWithText("New Connection").assertIsDisplayed()
        compose.onNodeWithTag("host-form-save").assertIsNotEnabled()
        compose.onNodeWithContentDescription("Close").performClick() // The leading icon is a close (X) here.
        compose.runOnIdle {
            assertEquals(1, closed)
            assertEquals(0, saved)
        }
    }
}
