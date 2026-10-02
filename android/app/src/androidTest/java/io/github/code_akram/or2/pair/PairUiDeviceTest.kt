package io.github.code_akram.or2.pair

import android.graphics.Bitmap
import androidx.activity.compose.setContent
import androidx.compose.runtime.key
import androidx.compose.ui.graphics.asAndroidBitmap
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.assertTextContains
import androidx.compose.ui.test.assertTextEquals
import androidx.compose.ui.test.captureToImage
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onRoot
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollTo
import androidx.compose.ui.test.performTextInput
import androidx.test.platform.app.InstrumentationRegistry
import io.github.code_akram.or2.MainActivity
import io.github.code_akram.or2.assertTouchTargetAtLeast
import io.github.code_akram.or2.data.KeyRecord
import io.github.code_akram.or2.ffi.parsePairPayload
import io.github.code_akram.or2.ui.Or2Theme
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import java.io.File

/**
 * The Easy pair screens from fabricated state: no camera, no network, no storage (the camera needs the phone;
 * the QR decoder, the flow and the pairing are covered by JVM tests). They compile and run on a device, and the
 * Easy pair, review and pairing screens leave a screenshot each in `files/pair-review/` of the app under test.
 */
class PairUiDeviceTest {
    @get:Rule val compose = createAndroidComposeRule<MainActivity>()

    private var generations = 0

    /** The phone's code in the contract's own example; its check character is valid. */
    private val shownCode = "7KQ4-M2XD-9PTM"
    private val hostKey = "&hk=ssh-ed25519%20AAAAC3NzaC1lZDI1NTE5AAAAIAc39XUWT33SvSLy6vA7I83%2BXgmwnHmYtMQRjLeaZ2U7"
    private val code = "or2-pair:2?name=Work%20Mac&user=alice&port=22&a=192.168.1.20&a=work-mac.local$hostKey&id=abcdefghijklm"
    private val manualCode = "or2-pair:2?name=Work%20Mac&user=alice&port=22&a=192.168.1.20&a=work-mac.local$hostKey"
    private val fingerprint = "SHA256:PK/nvGiFusFK9/6Qf8dSOX99mI5XQYiMAML2JHCjgQI"
    private val key = KeyRecord("k1", "Fixture key", "ssh-ed25519", "ssh-ed25519 AAAA", "SHA256:fixturefixturefixturefixture", "", byteArrayOf(), byteArrayOf())

    private fun show(content: @androidx.compose.runtime.Composable () -> Unit) = compose.runOnUiThread {
        val generation = ++generations
        compose.activity.setContent { key(generation) { Or2Theme { content() } } }
    }

    private fun shoot(name: String) {
        compose.waitForIdle()
        val bitmap = compose.onRoot().captureToImage().asAndroidBitmap()
        val directory = File(InstrumentationRegistry.getInstrumentation().targetContext.filesDir, "pair-review").apply { mkdirs() }
        File(directory, "$name.png").outputStream().use { assertTrue(bitmap.compress(Bitmap.CompressFormat.PNG, 100, it)) }
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
    fun theScannerShowsTheCodeToTypeOnTheHostTheCommandToCopyAndThenTheCamera() {
        var asked = 0
        show { PairScanScreen(shownCode, null, CameraAccess(granted = false, denied = false) { asked++ }, {}, back = {}) }
        compose.onNodeWithTag("pair-code").assertIsDisplayed().assertTextEquals(shownCode)
        compose.onNodeWithTag("pair-command").assertIsDisplayed().assertTextEquals("or2-pair")
        compose.onNodeWithTag("pair-copy-command").assertIsDisplayed().assertTouchTargetAtLeast().performClick()
        compose.onNodeWithTag("pair-camera").performScrollTo().assertIsDisplayed()
        shoot("easy-pair")
        // No or2-pair yet: the installer one-liner, to copy.
        compose.onNodeWithTag("pair-install-command").performScrollTo().assertIsDisplayed()
            .assertTextEquals("curl -fsSL https://raw.githubusercontent.com/code-akram/or2/main/scripts/install-or2-pair.sh | sh")
        compose.onNodeWithTag("pair-copy-install").performScrollTo().assertTouchTargetAtLeast().performClick()
    }

    @Test
    fun theScannerExplainsTheCameraAndPastingHandsOverTheCode() {
        var asked = 0
        val received = mutableListOf<String>()
        show { PairScanScreen(shownCode, "That is not an or2 pairing code.", CameraAccess(granted = false, denied = false) { asked++ }, { received += it }, back = {}) }
        // The screen asks once by itself when it opens; the button asks again.
        compose.runOnIdle { assertEquals(1, asked) }
        compose.onNodeWithTag("pair-allow-camera").performScrollTo().assertIsDisplayed().assertTouchTargetAtLeast().performClick()
        compose.runOnIdle { assertEquals(2, asked) }
        compose.onNodeWithTag("pair-scan-error").performScrollTo().assertTextContains("not an or2 pairing code", substring = true)
        compose.onNodeWithTag("pair-paste").performScrollTo().performClick()
        compose.onNodeWithTag("pair-paste-continue").performScrollTo().assertIsNotEnabled()
        compose.onNodeWithTag("pair-paste-field").performScrollTo().performTextInput(code)
        compose.onNodeWithTag("pair-paste-continue").performScrollTo().assertIsEnabled().performClick()
        assertEquals(listOf(code), received)
    }

    @Test
    fun aDeniedCameraOffersOnlyPasting() {
        show { PairScanScreen(shownCode, null, CameraAccess(granted = false, denied = true) {}, {}, back = {}) }
        compose.onNodeWithTag("pair-camera").performScrollTo().assertIsDisplayed().assertTextContains("not allowed", substring = true)
        compose.onNodeWithTag("pair-paste").performScrollTo().assertIsDisplayed()
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
        // The host installs the key for the account that ran or2-pair: the user is read-only.
        compose.onNodeWithTag("pair-username").performScrollTo().assertIsNotEnabled()
        shoot("review")
        compose.onNodeWithTag("pair-key-new").performScrollTo().performClick()
        assertTrue(edits >= 1)
        compose.onNodeWithTag("pair-submit").performScrollTo().assertIsEnabled().assertTextEquals("Pair").performClick()
        assertEquals(1, submitted)
    }

    @Test
    fun afterTheHostAcceptedAKeyTheRetryCannotChangeIt() {
        val offer = parsePairPayload(code)
        var edits = 0
        show {
            PairReviewScreen(
                PairReview(
                    offer, offer.name, offer.username, KeyChoice.Existing("k1"),
                    error = "The host accepted the key, but this phone could not save the host. Try again.",
                    accepted = AcceptedKey("k1", key.fingerprint),
                ),
                listOf(key), edit = { _, _, _ -> edits++ }, submit = {}, back = {},
            )
        }
        // The host installed k1: neither another key nor a new one can be picked, and the retry only saves.
        compose.onNodeWithTag("pair-key:k1").performScrollTo().assertIsNotEnabled()
        compose.onNodeWithTag("pair-key-new").performScrollTo().assertIsNotEnabled()
        compose.onNodeWithTag("pair-key-note").performScrollTo().assertTextContains("only saving the host is left", substring = true)
        compose.onNodeWithTag("pair-submit").performScrollTo().assertIsEnabled()
        assertEquals(0, edits)
    }

    @Test
    fun aManualCodeLetsYouNameTheUserAndAddsTheHostWithoutPairing() {
        val offer = parsePairPayload(manualCode)
        show {
            PairReviewScreen(PairReview(offer, offer.name, offer.username, KeyChoice.New), emptyList(), edit = { _, _, _ -> }, submit = {}, back = {})
        }
        compose.onNodeWithTag("pair-username").performScrollTo().assertIsEnabled()
        compose.onNodeWithTag("pair-submit").performScrollTo().assertIsEnabled().assertTextEquals("Add host")
    }

    @Test
    fun aWorkingReviewDisablesTheButtonAndAFailureShowsItsReason() {
        val offer = parsePairPayload(code)
        show {
            PairReviewScreen(
                PairReview(offer, "", offer.username, KeyChoice.New, error = "Couldn't reach Work Mac on port 22.", working = false),
                emptyList(), edit = { _, _, _ -> }, submit = {}, back = {},
            )
        }
        compose.onNodeWithTag("pair-error").performScrollTo().assertTextContains("Couldn't reach", substring = true)
        compose.onNodeWithTag("pair-submit").performScrollTo().assertIsNotEnabled() // No name yet.
    }

    @Test
    fun theProgressScreenNamesTheHostBeingPairedWith() {
        var cancelled = 0
        show { PairProgressScreen("Work Mac", cancel = { cancelled++ }) }
        compose.onNodeWithTag("pair-progress-title").assertIsDisplayed().assertTextEquals("Pairing with Work Mac…")
        shoot("pairing")
        compose.onNodeWithTag("pair-cancel").assertTouchTargetAtLeast().performClick()
        assertEquals(1, cancelled)
    }

    @Test
    fun theBatteryStepExplainsInOneLineAndAllowNotNowAndBackAnswerIt() {
        val answers = mutableListOf<String>()
        show { KeepAliveScreen(waiting = false, allow = { answers += "allow" }, notNow = { answers += "not-now" }) }
        compose.onNodeWithTag("keepalive-title").assertIsDisplayed().assertTextEquals(KEEP_ALIVE_TITLE)
        compose.onNodeWithTag("keepalive-why").assertIsDisplayed().assertTextEquals(KEEP_ALIVE_WHY)
        shoot("keepalive")
        compose.onNodeWithTag("keepalive-allow").assertIsEnabled().performClick()
        compose.onNodeWithTag("keepalive-not-now").assertIsEnabled().performClick()
        compose.runOnUiThread { compose.activity.onBackPressedDispatcher.onBackPressed() } // Back is "Not now".
        compose.runOnIdle { assertEquals(listOf("allow", "not-now", "not-now"), answers) }
    }

    @Test
    fun whileAndroidsBatteryDialogIsUpTheStepAnswersNothing() {
        val answers = mutableListOf<String>()
        show { KeepAliveScreen(waiting = true, allow = { answers += "allow" }, notNow = { answers += "not-now" }) }
        compose.onNodeWithTag("keepalive-allow").assertIsNotEnabled()
        compose.onNodeWithTag("keepalive-not-now").assertIsNotEnabled()
        compose.runOnUiThread { compose.activity.onBackPressedDispatcher.onBackPressed() }
        compose.runOnIdle { assertEquals(emptyList<String>(), answers) }
    }

    @Test
    fun aManualCodeEndsOnTheKeyToInstall() {
        var done = 0
        show { PairInstallKeyScreen("Work Mac", "ssh-ed25519 AAAA phone", "SHA256:abc", done = { done++ }) }
        compose.onNodeWithTag("pair-install-key").assertTextContains("ssh-ed25519 AAAA phone")
        compose.onNodeWithTag("pair-install-done").performScrollTo().performClick()
        assertEquals(1, done)
    }
}
