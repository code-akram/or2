package io.github.code_akram.or2.host

import androidx.activity.compose.setContent
import androidx.compose.runtime.key
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.assertTextEquals
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollTo
import androidx.compose.ui.test.performTextInput
import io.github.code_akram.or2.MainActivity
import io.github.code_akram.or2.connection.UDP_BLOCKED_LINE
import io.github.code_akram.or2.connection.uiHost
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.HostEndpoint
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.HerdrSessionInfo
import io.github.code_akram.or2.ffi.HostCapabilities
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.PublicKeyInfo
import io.github.code_akram.or2.ffi.SessionFailure
import io.github.code_akram.or2.ffi.TmuxSession
import io.github.code_akram.or2.ui.Or2Theme
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test

/**
 * The host screen and its session picker sheet with fabricated state: no connection, Keystore or
 * database is touched. The sheet never opens by itself: "Open a session" opens it on a connected host.
 */
class HostScreenUiDeviceTest {
    @get:Rule val compose = createAndroidComposeRule<MainActivity>()

    private val host = uiHost(4, "Build box", addresses = listOf(HostEndpoint("a.invalid", 22), HostEndpoint("b.invalid", 2222)))
    private val caps = HostCapabilities("/usr/bin/tmux", "/usr/bin/herdr", null, "C.UTF-8", listOf(
        HerdrSessionInfo("default", true, true), HerdrSessionInfo("work", true, false), HerdrSessionInfo("old", false, false)))
    private val tmux = TmuxList.Loaded(listOf(TmuxSession("main", 3u, 1u, 0L, 20L), TmuxSession("build", 1u, 0u, 0L, 10L)))

    private var generations = 0

    private class Calls {
        val log = mutableListOf<String>()
    }

    private fun show(
        state: HostState?, caps: HostCapabilities? = this.caps, tmux: TmuxList = this.tmux, capsError: String? = null,
        calls: Calls = Calls(), busy: Boolean = false, host: Host = this.host, terminals: List<HostTerminalItem> = emptyList(),
        udpBlocked: Boolean = false,
    ) = compose.runOnUiThread {
        val generation = ++generations
        compose.activity.setContent {
            // A new key per call: remember/rememberSaveable state must not leak from the previous show().
            key(generation) { Or2Theme {
                HostScreen(host, state, caps, capsError, tmux, busy,
                    connect = { calls.log += "connect" }, disconnect = { calls.log += "disconnect" },
                    approve = { calls.log += "approve:${it.presented.fingerprint}" }, reject = { calls.log += "reject" },
                    openShell = { calls.log += "shell" }, openTmux = { calls.log += "tmux:$it" },
                    openHerdr = { calls.log += "herdr:$it" }, refresh = { calls.log += "refresh" },
                    terminals = terminals, resume = { calls.log += "resume:$it" }, edit = { calls.log += "edit" }, back = { calls.log += "back" },
                    udpBlocked = udpBlocked)
            } }
        }
    }

    private fun pickerTab(index: Int) = compose.onNodeWithTag("picker-tab:$index").performClick()

    /** "Open a session" opens the sheet (choosing a session closes it again). */
    private fun openPicker() {
        compose.onNodeWithTag("host-open-picker").performScrollTo().performClick()
        compose.onNodeWithTag("session-picker").assertIsDisplayed()
    }

    @Test
    fun aBlockedUdpVerdictShowsOneMutedLineOnlyWhileConnected() {
        show(HostState.Connected(0u), udpBlocked = true)
        compose.onNodeWithTag("host-udp-blocked").assertExists().assertTextEquals(UDP_BLOCKED_LINE)
        show(HostState.Connected(0u), udpBlocked = false)
        compose.onNodeWithTag("host-udp-blocked").assertDoesNotExist()
        show(null, udpBlocked = true)
        compose.onNodeWithTag("host-udp-blocked").assertDoesNotExist()
    }

    @Test
    fun aDisconnectedHostOffersUnlockAndConnectAndNoPicker() {
        val calls = Calls()
        show(null, calls = calls)
        compose.onNodeWithText("Build box").assertIsDisplayed()
        compose.onNodeWithText("fixture-user@a.invalid:22, b.invalid:2222").assertIsDisplayed()
        compose.onNodeWithTag("host-connect").assertIsEnabled().performClick()
        compose.runOnIdle { assertEquals(listOf("connect"), calls.log) }
        compose.onNodeWithTag("session-picker").assertDoesNotExist()
        compose.onNodeWithTag("host-shell").assertDoesNotExist()
        compose.onNodeWithTag("host-open-picker").assertDoesNotExist()
    }

    @Test
    fun aHostWithoutAKeyCannotConnect() {
        show(null, host = uiHost(5, "Keyless", keyId = null))
        compose.onNodeWithTag("host-connect").assertIsNotEnabled()
        compose.onNodeWithText("Select a key first").assertIsDisplayed()
    }

    @Test
    fun aFailedConnectionExplainsItselfAndCanRetry() {
        show(HostState.Closed(CloseReason.Failed(SessionFailure.AuthenticationRejected)))
        compose.onNodeWithText("Authentication rejected. Check the username and public-key authorization.").assertIsDisplayed()
        compose.onNodeWithTag("host-connect").assertIsEnabled()
    }

    @Test
    fun theHostKeyPromptIsAnsweredOnTheHostScreen() {
        val calls = Calls()
        val prompt = HostState.AwaitingHostKeyDecision(PublicKeyInfo("ssh-ed25519", "k", "SHA256:presented", ""), emptyList())
        show(prompt, calls = calls)
        compose.onNodeWithTag("host-state").assertIsDisplayed()
        compose.onNodeWithText("Trust this host key?").assertIsDisplayed()
        compose.onNodeWithText("SHA256:presented").assertIsDisplayed()
        compose.onNodeWithTag("hostkey-approve").performClick()
        compose.runOnIdle { assertEquals(listOf("approve:SHA256:presented"), calls.log) }
        show(prompt, calls = calls)
        compose.onNodeWithTag("hostkey-reject").performClick()
        compose.runOnIdle { assertEquals("reject", calls.log.last()) }
    }

    @Test
    fun aConnectedHostScreenDoesNotCoverItselfWithThePicker() {
        show(HostState.Connected(0u))
        compose.onNodeWithTag("host-detail").assertIsDisplayed()
        compose.onNodeWithTag("session-picker").assertDoesNotExist()
        compose.onNodeWithTag("host-open-picker").assertIsDisplayed()
        // A host that connects while the screen is up does not open it either.
        show(HostState.Connecting)
        compose.onNodeWithTag("session-picker").assertDoesNotExist()
    }

    @Test
    fun aConnectedHostOpensThePickerWithHerdrTmuxAndAPlainShell() {
        val calls = Calls()
        show(HostState.Connected(1u), calls = calls)
        compose.onNodeWithText("Using address 2: b.invalid:2222").assertExists()
        openPicker()
        // herdr is the first segment: the default session opens without a name, a named one by
        // name, and a stopped one not at all.
        compose.onNodeWithTag("herdr-open:old").assertIsNotEnabled()
        compose.onNodeWithTag("herdr-open:default").performClick()
        openPicker()
        compose.onNodeWithTag("herdr-open:work").performClick()
        openPicker()
        pickerTab(1)
        compose.onNodeWithText("3 windows · 1 attached").assertIsDisplayed()
        compose.onNodeWithTag("tmux-attach:build").performClick()
        openPicker()
        compose.onNodeWithTag("host-shell").assertTextEquals("Shell").performClick() // A plain shell.
        openPicker()
        pickerTab(1)
        compose.onNodeWithTag("host-refresh").performScrollTo().performClick()
        compose.runOnIdle { assertEquals(listOf("herdr:null", "herdr:work", "tmux:build", "shell", "refresh"), calls.log) }
        compose.onNodeWithTag("session-picker").assertIsDisplayed() // Refreshing keeps the sheet open.
    }

    @Test
    fun theRecentTabResumesOpenTerminalsOfThisHost() {
        val calls = Calls()
        show(HostState.Connected(0u), calls = calls, terminals = listOf(HostTerminalItem(7, "tmux main", false), HostTerminalItem(8, "shell", true)))
        openPicker()
        pickerTab(2)
        compose.onNodeWithTag("recent-open:7").performClick()
        compose.runOnIdle { assertEquals(listOf("resume:7"), calls.log) }
        compose.onNodeWithTag("open-terminal:8").performScrollTo().assertIsDisplayed() // Also listed on the host screen.
        show(HostState.Connected(0u))
        openPicker()
        pickerTab(2)
        compose.onNodeWithTag("recent-empty").assertIsDisplayed()
    }

    @Test
    fun theSheetCanBeDismissedAndTheHostCanBeDisconnectedAfterwards() {
        val calls = Calls()
        show(HostState.Connected(0u), calls = calls)
        openPicker()
        compose.onNodeWithTag("host-shell").performClick()
        compose.onNodeWithTag("host-disconnect").performScrollTo().assertIsDisplayed().performClick()
        compose.runOnIdle { assertEquals(listOf("shell", "disconnect"), calls.log) }
    }

    @Test
    fun newTmuxSessionNamesAreValidatedBeforeCreating() {
        val calls = Calls()
        show(HostState.Connected(0u), calls = calls)
        openPicker()
        pickerTab(1)
        compose.onNodeWithTag("tmux-new").performScrollTo().assertIsNotEnabled()
        compose.onNodeWithTag("tmux-new-name").performScrollTo().performTextInput("a:b")
        compose.onNodeWithText("Remove backslash, colon and dot characters.").assertIsDisplayed()
        compose.onNodeWithTag("tmux-new").assertIsNotEnabled()
        compose.onNodeWithTag("tmux-new-name").performTextInput("x") // "a:bx" is still invalid.
        compose.onNodeWithTag("tmux-new").assertIsNotEnabled()
        show(HostState.Connected(0u), calls = calls)
        openPicker()
        pickerTab(1)
        compose.onNodeWithTag("tmux-new-name").performScrollTo().performTextInput("my work")
        compose.onNodeWithTag("tmux-new").performScrollTo().assertIsEnabled().performClick()
        compose.runOnIdle { assertEquals(listOf("tmux:my work"), calls.log) }
    }

    @Test
    fun missingTmuxHerdrAndFailedListingsAreExplainedNotHidden() {
        show(HostState.Connected(0u), caps = caps.copy(tmux = null, herdr = null))
        openPicker()
        compose.onNodeWithTag("herdr-missing").assertIsDisplayed()
        pickerTab(1)
        compose.onNodeWithTag("tmux-missing").assertIsDisplayed()
        show(HostState.Connected(0u), tmux = TmuxList.Failed("The connection has closed. Reconnect to continue."))
        openPicker()
        pickerTab(1)
        compose.onNodeWithText("The connection has closed. Reconnect to continue.").assertIsDisplayed()
        show(HostState.Connected(0u), caps = null, tmux = TmuxList.Loading)
        openPicker()
        compose.onNodeWithText("Checking the host…", substring = true).assertIsDisplayed()
        show(HostState.Connected(0u), capsError = "probe failed")
        openPicker()
        compose.onNodeWithText("Could not query the host. Try Refresh.").assertIsDisplayed()
    }

    /** The picker as Home's session button shows it, before its host has connected ([gate]). */
    private fun showGate(gate: PickerGate?, actions: MutableList<GateAction>) = compose.runOnUiThread {
        val generation = ++generations
        compose.activity.setContent {
            key(generation) { Or2Theme {
                SessionPickerSheet(caps, null, tmux, emptyList(), {}, {}, {}, {}, {}, {}, gate = gate, gateAction = { actions += it })
            } }
        }
    }

    @Test
    fun beforeItsHostConnectsThePickerShowsProgressThenWhyNotWithOneAction() {
        val actions = mutableListOf<GateAction>()
        showGate(PickerGate.Connecting("Build box", "Checking server…", spinning = true), actions)
        compose.onNodeWithTag("session-picker").assertIsDisplayed()
        compose.onNodeWithTag("picker-progress").assertTextEquals("Checking server…")
        compose.onNodeWithTag("picker-spinner").assertIsDisplayed()
        compose.onNodeWithTag("picker-tab:0").assertDoesNotExist() // No lists until the host is connected.
        compose.onNodeWithTag("host-shell").assertDoesNotExist()
        showGate(PickerGate.Stopped("Build box", "Authentication rejected.", failed = true, detail = "a.invalid:22 · refused",
            action = GateAction.RETRY, enabled = true), actions)
        compose.onNodeWithTag("picker-reason").assertTextEquals("Authentication rejected.")
        compose.onNodeWithTag("picker-detail").assertIsDisplayed()
        compose.onNodeWithTag("picker-retry").assertTextEquals("Retry").performClick()
        showGate(PickerGate.Stopped("Build box", "Not connected", false, null, GateAction.CONNECT, enabled = false), actions)
        compose.onNodeWithTag("picker-retry").assertIsNotEnabled()
        compose.runOnIdle { assertEquals(listOf(GateAction.RETRY), actions) }
        // Connected: the gate is gone and the lists show in the same sheet.
        showGate(null, actions)
        compose.onNodeWithTag("picker-gate").assertDoesNotExist()
        compose.onNodeWithTag("herdr-open:work").assertIsDisplayed()
    }

    @Test
    fun aHostWithoutHerdrOpensOnTheTmuxSegment() {
        show(HostState.Connected(0u), caps = caps.copy(herdr = null, herdrSessions = emptyList()))
        openPicker()
        compose.onNodeWithTag("tmux-attach:main").assertIsDisplayed()
    }
}
