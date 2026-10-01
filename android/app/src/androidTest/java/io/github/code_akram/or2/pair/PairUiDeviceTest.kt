package io.github.code_akram.or2.pair

import androidx.activity.compose.setContent
import androidx.compose.runtime.key
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.assertTextContains
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollTo
import androidx.compose.ui.test.performTextInput
import io.github.code_akram.or2.MainActivity
import io.github.code_akram.or2.assertTouchTargetAtLeast
import io.github.code_akram.or2.data.KeyRecord
import io.github.code_akram.or2.ffi.parsePairPayload
import io.github.code_akram.or2.ui.Or2Theme
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

/**
 * The Easy pair screens from fabricated state: no camera, no network, no storage (the camera needs the phone;
 * the QR decoder, the flow and the exchange are covered by JVM tests). They compile and run on a device.
 */
class PairUiDeviceTest {
    @get:Rule val compose = createAndroidComposeRule<MainActivity>()

    private var generations = 0
    private val code = "or2-pair:1?name=Work%20Mac&user=alice&port=22&a=192.168.1.20&a=work-mac.local" +
        "&hk=ssh-ed25519%20AAAAC3NzaC1lZDI1NTE5AAAAIAc39XUWT33SvSLy6vA7I83%2BXgmwnHmYtMQRjLeaZ2U7" +
        "&pair=192.168.1.20:41234&otp=AAAQEAYEAUDAOCAJBIFQYDIOB4"
    private val fingerprint = "SHA256:PK/nvGiFusFK9/6Qf8dSOX99mI5XQYiMAML2JHCjgQI"
    private val key = KeyRecord("k1", "Fixture key", "ssh-ed25519", "ssh-ed25519 AAAA", "SHA256:fixturefixturefixturefixture", "", byteArrayOf(), byteArrayOf())

    private fun show(content: @androidx.compose.runtime.Composable () -> Unit) = compose.runOnUiThread {
        val generation = ++generations
        compose.activity.setContent { key(generation) { Or2Theme { content() } } }
    }

    @Test
    fun theAddHostSheetOffersEasyPairFirstAndTheManualFormSecond() {
        var chosen = ""
        show { AddHostSheet(easyPair = { chosen += "easy" }, manual = { chosen += "manual" }, dismiss = {}) }
        compose.onNodeWithTag("add-host-easy").assertIsDisplayed().assertTouchTargetAtLeast().assertTextContains("FASTEST", substring = true)
        compose.onNodeWithTag("add-host-manual").assertIsDisplayed().assertTextContains("Set up manually", substring = true)
        compose.onNodeWithTag("add-host-easy").performClick()
        compose.onNodeWithTag("add-host-manual").performClick()
        assertEquals("easymanual", chosen)
    }

    @Test
    fun theScannerExplainsTheCameraAndPastingHandsOverTheCode() {
        var asked = 0
        val received = mutableListOf<String>()
        show { PairScanScreen("That is not an or2 pairing code.", CameraAccess(granted = false, denied = false) { asked++ }, { received += it }, back = {}) }
        compose.onNodeWithTag("pair-allow-camera").assertIsDisplayed().assertTouchTargetAtLeast().performClick()
        assertEquals(1, asked)
        compose.onNodeWithTag("pair-scan-error").assertTextContains("not an or2 pairing code", substring = true)
        compose.onNodeWithTag("pair-paste").performScrollTo().performClick()
        compose.onNodeWithTag("pair-paste-continue").performScrollTo().assertIsNotEnabled()
        compose.onNodeWithTag("pair-paste-field").performScrollTo().performTextInput(code)
        compose.onNodeWithTag("pair-paste-continue").performScrollTo().assertIsEnabled().performClick()
        assertEquals(listOf(code), received)
    }

    @Test
    fun aDeniedCameraOffersOnlyPasting() {
        show { PairScanScreen(null, CameraAccess(granted = false, denied = true) {}, {}, back = {}) }
        compose.onNodeWithTag("pair-camera").assertIsDisplayed().assertTextContains("not allowed", substring = true)
        compose.onNodeWithTag("pair-paste").assertIsDisplayed()
    }

    @Test
    fun theReviewShowsTheHostTheAddressesTheKeyFingerprintAndWhichKeyIsAuthorized() {
        val offer = parsePairPayload(code)
        var review = PairReview(offer, offer.name, offer.username, KeyChoice.Existing("k1"))
        var edits = 0
        var submitted = 0
        show {
            PairReviewScreen(review, listOf(key), edit = { _, _, choice -> edits++; if (choice != null) review = review.copy(choice = choice) }, submit = { submitted++ }, back = {})
        }
        compose.onNodeWithTag("pair-host-fingerprint").performScrollTo().assertTextContains(fingerprint)
        compose.onNodeWithTag("pair-addresses").performScrollTo().assertTextContains("192.168.1.20:22", substring = true)
        compose.onNodeWithTag("pair-key:k1").performScrollTo().assertIsDisplayed().assertTouchTargetAtLeast()
        compose.onNodeWithTag("pair-key-new").performScrollTo().performClick()
        assertTrue(edits >= 1)
        compose.onNodeWithTag("pair-submit").performScrollTo().assertIsEnabled().assertTextContains("Pair and add host").performClick()
        assertEquals(1, submitted)
    }

    @Test
    fun aWorkingReviewDisablesTheButtonAndAFailureShowsItsReason() {
        val offer = parsePairPayload(code)
        show {
            PairReviewScreen(
                PairReview(offer, "", offer.username, KeyChoice.New, error = "The host declined the key.", working = false),
                emptyList(), edit = { _, _, _ -> }, submit = {}, back = {},
            )
        }
        compose.onNodeWithTag("pair-error").performScrollTo().assertTextContains("declined", substring = true)
        compose.onNodeWithTag("pair-submit").performScrollTo().assertIsNotEnabled() // No name yet.
    }

    @Test
    fun theProgressScreenShowsTheFingerprintTheHostsUserShouldCompare() {
        var cancelled = 0
        show { PairProgressScreen("alice", "SHA256:phonephonephonephone", cancel = { cancelled++ }) }
        compose.onNodeWithTag("pair-phone-fingerprint").assertIsDisplayed().assertTextContains("SHA256:phonephonephonephone")
        compose.onNodeWithTag("pair-cancel").assertTouchTargetAtLeast().performClick()
        assertEquals(1, cancelled)
    }

    @Test
    fun aCodeWithoutAListenerEndsOnTheKeyToInstall() {
        var done = 0
        show { PairInstallKeyScreen("Work Mac", "ssh-ed25519 AAAA phone", "SHA256:abc", done = { done++ }) }
        compose.onNodeWithTag("pair-install-key").assertTextContains("ssh-ed25519 AAAA phone")
        compose.onNodeWithTag("pair-install-done").performScrollTo().performClick()
        assertEquals(1, done)
    }
}
