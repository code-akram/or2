package io.github.code_akram.or2.home

import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.assertTextEquals
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.longClick
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollTo
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.unit.dp
import io.github.code_akram.or2.MainActivity
import io.github.code_akram.or2.assertTouchTargetAtLeast
import io.github.code_akram.or2.connection.HostConnections
import io.github.code_akram.or2.connection.UiPort
import io.github.code_akram.or2.connection.UiTrust
import io.github.code_akram.or2.connection.uiHost
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.HostEndpoint
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.SessionFailure
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.inbox.linkStatus
import io.github.code_akram.or2.terminal.TerminalThumbnail
import io.github.code_akram.or2.terminal.Transport
import io.github.code_akram.or2.ui.Or2Theme
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

/** The Home screen with fabricated state: no connection, Keystore or database is touched. */
class HomeUiDeviceTest {
    @get:Rule val compose = createAndroidComposeRule<MainActivity>()

    private val calls = mutableListOf<String>()

    private fun card(host: Host, state: HostState?, blocked: Int = 0, unlocking: Boolean = false) =
        HostCard(host, hostCardStatus(state, unlocking, blocked, host.sleeps, host.addresses), linkStatus(state, host.sleeps))

    private fun show(
        hosts: List<HostCard>, sessions: List<HomeSession> = emptyList(), keyCount: Int = 1, blocked: Int = 0, working: Int = 0,
        canConnectAll: Boolean = false, busy: Boolean = false, resume: HomeResume? = null, batteryCard: Boolean = false,
    ) = compose.runOnUiThread {
        compose.activity.setContent {
            Or2Theme {
                HomeScreen(
                    sessions, hosts, keyCount, blocked, working, canConnectAll, busy,
                    openSession = { calls += "session:${it.id}" }, openHost = { calls += "open:${it.id}" },
                    addHost = { calls += "add" }, editHost = { calls += "edit:${it.id}" }, connectHost = { calls += "connect:${it.id}" },
                    disconnectHost = { calls += "disconnect:${it.id}" }, deleteHost = { calls += "delete:${it.id}" },
                    openInbox = { calls += "inbox" }, openKeys = { calls += "keys" }, connectAll = { calls += "all" },
                    openAbout = { calls += "about" },
                    resume = resume, onResume = { calls += "resume" },
                    batteryCard = batteryCard, allowBattery = { calls += "allow-battery" }, dismissBattery = { calls += "dismiss-battery" },
                )
            }
        }
    }

    private val one = uiHost(1, "Alpha", addresses = listOf(HostEndpoint("alpha.invalid", 22)))
    private val two = uiHost(2, "Beta", addresses = listOf(HostEndpoint("beta.invalid", 2222), HostEndpoint("beta2.invalid", 22)))

    @Test
    fun hostCardsShowTheAddressProgressAndFailureInPlace() {
        show(listOf(
            card(one, HostState.Connected(0u), blocked = 1),
            card(two, HostState.Connecting),
            card(uiHost(3, "Gamma"), HostState.Closed(CloseReason.Failed(SessionFailure.AuthenticationRejected))),
            card(uiHost(4, "Delta"), null, unlocking = true),
            card(uiHost(5, "Eps"), null),
        ))
        compose.onNodeWithTag("host:1").assertIsDisplayed()
        compose.onNodeWithText("fixture-user@alpha.invalid:22").assertIsDisplayed()
        // The card is one tappable node; its parts are checked in the unmerged tree.
        compose.onNodeWithTag("host-dot:1", useUnmergedTree = true).assertIsDisplayed() // An attention dot: a blocked agent.
        // Progress replaces the address line, with a spinner in the icon slot and no dot.
        compose.onNodeWithTag("host-progress:2", useUnmergedTree = true).assertTextEquals("Checking server…")
        compose.onNodeWithTag("host-spinner:2", useUnmergedTree = true).assertIsDisplayed()
        compose.onNodeWithTag("host-dot:2", useUnmergedTree = true).assertDoesNotExist()
        compose.onNodeWithText("fixture-user@beta.invalid:2222 +1").assertDoesNotExist()
        compose.onNodeWithTag("host-failure:3", useUnmergedTree = true).assertIsDisplayed()
        compose.onNodeWithText("Authentication rejected", substring = true).assertIsDisplayed()
        compose.onNodeWithTag("host-progress:4", useUnmergedTree = true).assertTextEquals("Unlocking key…")
        compose.onNodeWithTag("host:5").performScrollTo().assertIsDisplayed()
        compose.onNodeWithText("fixture-user@fixture.invalid:22").assertIsDisplayed() // The default uiHost address.
    }

    @Test
    fun theBatteryCardIsSmallDismissibleAndLeavesTheHostListUsable() {
        show(listOf(card(one, null)), batteryCard = true)
        compose.onNodeWithTag("home-battery-card").assertIsDisplayed()
        compose.onNodeWithText("Background connections may drop").assertIsDisplayed()
        // Nothing is blocked: the list is there and a host can still be opened.
        compose.onNodeWithTag("host:1").assertIsDisplayed().performClick()
        compose.onNodeWithTag("battery-card-allow").performClick()
        compose.onNodeWithTag("battery-card-dismiss").performClick()
        compose.runOnIdle { assertEquals(listOf("open:1", "allow-battery", "dismiss-battery"), calls) }
    }

    @Test
    fun noBatteryCardWhenTheExemptionIsInPlaceOrNeverDeclined() {
        show(listOf(card(one, null)), batteryCard = false)
        compose.onNodeWithTag("home-battery-card").assertDoesNotExist()
    }

    @Test
    fun anUnreachableHostSaysWhatEachAddressDidInMutedMonoLines() {
        val mac = uiHost(6, "Mac", addresses = listOf(HostEndpoint("mac.local", 22), HostEndpoint("198.51.100.20", 22)))
        val unreachable = HostState.Closed(
            CloseReason.Failed(
                SessionFailure.Unreachable("TCP connection failed: address 0: name not resolved (mDNS) after 3 tries; address 1: no answer within 6 s"),
            ),
        )
        show(listOf(card(mac, unreachable)))
        compose.onNodeWithTag("host-failure:6", useUnmergedTree = true).assertIsDisplayed()
        compose.onNodeWithTag("host-detail-lines:6", useUnmergedTree = true).assertIsDisplayed()
        compose.onNodeWithText("mac.local:22 \u00b7 name not resolved (mDNS) after 3 tries", substring = true, useUnmergedTree = true).assertIsDisplayed()
        compose.onNodeWithText("198.51.100.20:22 \u00b7 no answer within 6 s", substring = true, useUnmergedTree = true).assertIsDisplayed()
    }

    @Test
    fun tapOpensAHostAndLongPressOffersEditConnectDisconnectAndDelete() {
        show(listOf(card(one, null), card(two, HostState.Connected(0u))))
        compose.onNodeWithTag("host:1").performClick()
        compose.runOnIdle { assertEquals(listOf("open:1"), calls) }
        // A host that is not connected can be connected or edited, not disconnected.
        compose.onNodeWithTag("host:1").performTouchInput { longClick() }
        compose.onNodeWithTag("option-connect").assertIsDisplayed().performClick()
        compose.onNodeWithTag("host:1").performTouchInput { longClick() }
        compose.onNodeWithTag("option-disconnect").assertDoesNotExist()
        compose.onNodeWithTag("option-edit").performClick()
        // A connected one can be disconnected instead of connected.
        compose.onNodeWithTag("host:2").performTouchInput { longClick() }
        compose.onNodeWithTag("option-connect").assertDoesNotExist()
        compose.onNodeWithTag("option-disconnect").performClick()
        // Delete asks first.
        compose.onNodeWithTag("host:2").performTouchInput { longClick() }
        compose.onNodeWithTag("option-delete").performClick()
        compose.onNodeWithText("Delete host?").assertIsDisplayed()
        compose.runOnIdle { assertFalse("delete:2" in calls) }
        compose.onNodeWithTag("delete-confirm").performClick()
        compose.runOnIdle { assertEquals(listOf("open:1", "connect:1", "edit:1", "disconnect:2", "delete:2"), calls) }
    }

    @Test
    fun aHostWithoutAKeyCannotBeConnectedFromItsOptions() {
        show(listOf(card(uiHost(1, "Keyless", keyId = null), null)), keyCount = 0)
        compose.onNodeWithTag("home-add-key").performScrollTo().assertIsDisplayed()
        compose.onNodeWithTag("host:1").performTouchInput { longClick() }
        compose.onNodeWithTag("option-connect").assertIsNotEnabled()
    }

    @Test
    fun sessionsAreThumbnailCardsWithHostAndTransportPillsAndTapResumes() {
        val sessions = listOf(
            HomeSession(5, "Alpha", "tmux main", "~/code/or2", Transport.SSH) { Box(it) },
            HomeSession(6, "Beta", "herdr work", "~/src", Transport.MOSH) { Box(it) },
        )
        show(listOf(card(one, HostState.Connected(0u))), sessions)
        compose.onNodeWithTag("session-card:5").assertIsDisplayed()
        compose.onNodeWithTag("session-host:5", useUnmergedTree = true).assertTextEquals("Alpha")
        compose.onNodeWithTag("session-transport:5", useUnmergedTree = true).assertTextEquals("SSH")
        compose.onNodeWithTag("session-transport:6", useUnmergedTree = true).assertTextEquals("Mosh")
        compose.onNodeWithText("tmux main").assertIsDisplayed()
        compose.onNodeWithText("~/code/or2").assertIsDisplayed()
        compose.onNodeWithTag("session-card:6").performClick()
        compose.runOnIdle { assertEquals(listOf("session:6"), calls) }
    }

    @Test
    fun chipsNavButtonsAndTheFabAreWired() {
        show(listOf(card(one, HostState.Connected(0u)), card(two, null)), blocked = 2, working = 1, canConnectAll = true)
        compose.onNodeWithTag("chip-attention").performScrollTo().assertTextEquals("Needs attention: 2").performClick()
        compose.onNodeWithTag("chip-working").assertTextEquals("Working: 1")
        compose.onNodeWithTag("home-connect-all").performClick()
        compose.onNodeWithTag("inbox-badge").assertIsDisplayed()
        compose.onNodeWithTag("nav-inbox").performClick()
        compose.onNodeWithTag("nav-keys").performClick()
        compose.onNodeWithTag("nav-about").assertTouchTargetAtLeast().performClick()
        compose.onNodeWithTag("home-add-host").performClick()
        compose.runOnIdle { assertEquals(listOf("inbox", "all", "inbox", "keys", "about", "add"), calls) }
    }

    @Test
    fun withoutHostsHomeExplainsAndOffersTheFirstSteps() {
        show(emptyList(), keyCount = 0)
        compose.onNodeWithTag("home-empty").assertIsDisplayed()
        compose.onNodeWithText("No connections yet").assertIsDisplayed()
        compose.onNodeWithTag("home-add-key").performScrollTo().performClick()
        compose.onNodeWithTag("home-add-host-card").performScrollTo().performClick()
        compose.runOnIdle { assertEquals(listOf("keys", "add"), calls) }
    }

    @Test
    fun aThumbnailTakesFramesAndHoldsTheTerminalOpenUntilItLeavesComposition() {
        val session = io.github.code_akram.or2.connection.UiSession()
        val port = UiPort { session }
        val holder = HostConnections({ _, listener -> port.also { it.hostListener = listener } }, UiTrust(), worker = Dispatchers.Unconfined)
        var shown by mutableStateOf(true)
        compose.runOnUiThread {
            runBlocking { holder.connect(uiHost(), byteArrayOf(1)) }
            port.native = HostState.Connected(0u)
            val terminal = holder.openTerminal(holder.host(1)!!, TerminalTarget.Shell)
            compose.activity.setContent {
                Or2Theme { if (shown) TerminalThumbnail(terminal, holder, Modifier.size(200.dp)) else Box(Modifier.fillMaxSize()) }
            }
        }
        compose.waitUntil(5_000) {
            var taken = 0
            compose.runOnUiThread { taken = session.frameTakes }
            taken > 0 // It asked for a full frame and took it.
        }
        // Closing the terminal while the thumbnail shows it must not destroy the handle under it ...
        compose.runOnUiThread { holder.dismissTerminal(holder.terminals.value.single()) }
        compose.runOnIdle { assertFalse(session.destroyed) }
        // ... only once it is gone.
        compose.runOnUiThread { shown = false }
        compose.waitUntil(5_000) {
            var destroyed = false
            compose.runOnUiThread { destroyed = session.destroyed }
            destroyed
        }
        compose.runOnIdle {
            assertEquals(1, session.closes)
            assertTrue(holder.terminals.value.isEmpty())
        }
    }

    @Test
    fun theResumeCardNamesWhereTheUserWasAndTapResumesIt() {
        show(listOf(card(one, null)), resume = HomeResume("Alpha: herdr w1:p2", "Mosh"))
        compose.onNodeWithTag("home-resume").assertIsDisplayed()
        compose.onNodeWithText("Alpha: herdr w1:p2").assertIsDisplayed()
        compose.onNodeWithText("Mosh").assertIsDisplayed()
        compose.onNodeWithTag("home-resume").performClick()
        compose.runOnIdle { assertEquals(listOf("resume"), calls) }
    }

    @Test
    fun withNothingToResumeThereIsNoResumeCard() {
        show(listOf(card(one, HostState.Connected(0u))))
        compose.onNodeWithTag("home-resume").assertDoesNotExist()
    }

    @Test
    fun aSleepingHostsLostConnectionReadsAsleepInMutedTextNotAFailure() {
        val laptop = uiHost(6, "MacBook", sleeps = true)
        val lost = HostState.Closed(CloseReason.Failed(SessionFailure.ConnectionLost("reset")))
        show(listOf(card(laptop, lost), card(uiHost(7, "Server"), lost)))
        compose.onNodeWithTag("host-asleep:6", useUnmergedTree = true).assertTextEquals("Asleep")
        compose.onNodeWithTag("host-failure:6", useUnmergedTree = true).assertDoesNotExist()
        compose.onNodeWithTag("host-dot:6", useUnmergedTree = true).assertDoesNotExist()
        // A host that is not flagged keeps showing the failure.
        compose.onNodeWithTag("host-failure:7", useUnmergedTree = true).assertIsDisplayed()
        // A tap still connects it (the user knows it woke up), like any unconnected host.
        compose.onNodeWithTag("host:6").performTouchInput { longClick() }
        compose.onNodeWithTag("option-connect").assertIsDisplayed()
    }
}
