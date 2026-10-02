package io.github.code_akram.or2.app

import androidx.activity.compose.setContent
import androidx.compose.runtime.key
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollTo
import androidx.compose.ui.test.performTextInput
import io.github.code_akram.or2.MainActivity
import io.github.code_akram.or2.connection.HostConnections
import io.github.code_akram.or2.connection.UiPort
import io.github.code_akram.or2.connection.UiTrust
import io.github.code_akram.or2.connection.uiHost
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.KeyRecord
import io.github.code_akram.or2.ffi.PairCode
import io.github.code_akram.or2.ffi.PairOffer
import io.github.code_akram.or2.ffi.PairResult
import io.github.code_akram.or2.ffi.PublicKeyInfo
import io.github.code_akram.or2.ffi.pairNewCode
import io.github.code_akram.or2.ffi.parsePairPayload
import io.github.code_akram.or2.pair.PairBackend
import io.github.code_akram.or2.pair.PairFlow
import io.github.code_akram.or2.pair.PairState
import io.github.code_akram.or2.pair.PairStore
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

/**
 * The whole app (`Or2App`) over fakes: no network, no Keystore, no database. Adding a host ends on the battery step
 * (after Easy pair, before the paired host connects; after the manual form's save), and a connect never shows a
 * permission or battery dialog.
 */
class AddHostEndDeviceTest {
    @get:Rule val compose = createAndroidComposeRule<MainActivity>()

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
    private val key = KeyRecord("fixture-key", "Fixture key", "ssh-ed25519", "ssh-ed25519 AAAA", "SHA256:fixture", "", byteArrayOf(), byteArrayOf())
    private val paired = uiHost(id = 41, label = "Work Mac")
    private val connected = mutableListOf<List<Long>>()
    private val saved = mutableListOf<Host>()
    private val holder = HostConnections({ _, listener -> UiPort().also { it.hostListener = listener } }, UiTrust(), worker = Dispatchers.Unconfined)
    private var generations = 0

    @After
    fun stop() = scope.cancel()

    /** A pairing that enrols at once and stores the host as [paired]; codes and the parser are the real native ones. */
    private fun pairedFlow(): PairFlow {
        val backend = object : PairBackend {
            override fun newCode(): PairCode = pairNewCode()
            override fun parse(text: String): PairOffer = parsePairPayload(text)
            override suspend fun enroll(offer: PairOffer, code: PairCode, publicKeyLine: String, deviceLabel: String) =
                PairResult(offer.username, "SHA256:installed")
        }
        val store = object : PairStore {
            override suspend fun saveTrustedHost(host: Host, hostKey: PublicKeyInfo) = paired
        }
        val flow = PairFlow(backend, store, scope)
        val code = "or2-pair:2?name=Work%20Mac&user=alice&port=22&a=192.0.2.20" +
            "&hk=ssh-ed25519%20AAAAC3NzaC1lZDI1NTE5AAAAIAc39XUWT33SvSLy6vA7I83%2BXgmwnHmYtMQRjLeaZ2U7&id=abcdefghijklm"
        assertTrue(flow.onCode(code, listOf(key)))
        flow.submit(listOf(key), "Fixture phone") { _, _ -> error("no key is made in this test") }
        assertTrue(flow.state.value is PairState.Paired)
        return flow
    }

    private fun show(battery: BatteryPrompt, hosts: List<Host>, pair: PairFlow? = null) = compose.runOnUiThread {
        val generation = ++generations
        compose.activity.setContent {
            key(generation) {
                Or2App(
                    hosts, listOf(key), null, busy = false, holder,
                    AppActions(
                        saveHost = { host, _ -> saved += host }, deleteHost = {}, generateKey = { _, _ -> }, importKey = { _, _, _ -> },
                        deleteKey = {}, connect = { list -> connected += list.map { it.id } }, approve = { _, _ -> }, reject = {},
                        message = {}, battery = battery,
                        // The system's request opens and closes at once here (no system dialog in a test).
                        answerKeepAlive = { allow -> if (battery.answer(allow) && allow) battery.requestClosed() },
                        pair = pair, deviceLabel = "Fixture phone",
                    ),
                )
            }
        }
    }

    @Test
    fun easyPairEndsOnTheBatteryStepAndTheHostConnectsOnlyAfterIt() {
        val battery = BatteryPrompt(MemoryPrefStore()) { false }
        show(battery, listOf(paired), pairedFlow())
        compose.onNodeWithTag("keepalive").assertIsDisplayed()
        compose.onNodeWithTag("keepalive-title").assertIsDisplayed()
        compose.runOnIdle { assertEquals(emptyList<List<Long>>(), connected) } // No unlock while the step is up.
        compose.onNodeWithTag("keepalive-allow").performClick()
        compose.onNodeWithTag("keepalive").assertDoesNotExist()
        compose.runOnIdle { assertEquals(listOf(listOf(41L)), connected) }
        compose.onNodeWithTag("host-detail").assertIsDisplayed() // The paired host's page.
    }

    @Test
    fun anExemptAppPairsStraightIntoTheConnect() {
        show(BatteryPrompt(MemoryPrefStore()) { true }, listOf(paired), pairedFlow())
        compose.runOnIdle { assertEquals(listOf(listOf(41L)), connected) }
        compose.onNodeWithTag("keepalive").assertDoesNotExist()
    }

    @Test
    fun theManualFormsSaveEndsOnTheBatteryStepAndNotNowReturnsHomeWithTheCard() {
        val battery = BatteryPrompt(MemoryPrefStore()) { false }
        show(battery, emptyList())
        compose.onNodeWithTag("home-add-host-manual").performScrollTo().performClick()
        compose.onNodeWithTag("host-label").performScrollTo().performTextInput("Fixture")
        compose.onNodeWithTag("address-hostname:0").performScrollTo().performTextInput("fixture.invalid")
        compose.onNodeWithTag("host-username").performScrollTo().performTextInput("fixture-user")
        compose.onNodeWithTag("host-form-primary").performScrollTo().performClick()
        compose.onNodeWithTag("keepalive").assertIsDisplayed()
        compose.runOnIdle { assertEquals(1, saved.size) }
        compose.onNodeWithTag("keepalive-not-now").performClick()
        compose.onNodeWithTag("home-list").assertIsDisplayed()
        compose.runOnIdle {
            assertEquals(emptyList<List<Long>>(), connected) // The manual path never connects by itself.
            assertTrue(battery.card.value)
        }
    }

    @Test
    fun aConnectFromHomeShowsNoPermissionOrBatteryDialog() {
        val battery = BatteryPrompt(MemoryPrefStore()) { false } // Never asked, not exempt: the old flow asked here.
        show(battery, listOf(uiHost(id = 7, label = "Alpha")))
        compose.onNodeWithTag("host:7").performClick()
        compose.runOnIdle { assertEquals(listOf(listOf(7L)), connected) }
        compose.onNodeWithTag("keepalive").assertDoesNotExist()
        compose.runOnIdle { assertTrue(battery.shouldOffer()) } // Still for the next host that is added.
    }
}
