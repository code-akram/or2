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
import io.github.code_akram.or2.ffi.HerdrSessionInfo
import io.github.code_akram.or2.ffi.HostCapabilities
import io.github.code_akram.or2.ffi.TmuxSession
import io.github.code_akram.or2.ui.Or2Theme
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test

/**
 * The session picker sheet with fabricated state (no connection, Keystore or database is touched): herdr and tmux
 * tabs, the Shell pill, Refresh, a new tmux session, the gate before its host connects, sessions already open marked
 * `● Open`, and the UDP line under the tabs.
 */
class SessionPickerDeviceTest {
    @get:Rule val compose = createAndroidComposeRule<MainActivity>()

    private val caps = HostCapabilities("/usr/bin/tmux", "/usr/bin/herdr", null, listOf(
        HerdrSessionInfo("default", true, true), HerdrSessionInfo("work", true, false), HerdrSessionInfo("old", false, false)))
    private val tmux = TmuxList.Loaded(listOf(TmuxSession("main", 3u, 1u), TmuxSession("build", 1u, 0u)))

    private var generations = 0

    private fun show(
        calls: MutableList<String> = mutableListOf(), caps: HostCapabilities? = this.caps, tmux: TmuxList = this.tmux,
        capsError: String? = null, open: OpenSessions = OpenSessions(), gate: PickerGate? = null,
        actions: MutableList<GateAction> = mutableListOf(), udpBlocked: Boolean = false,
    ) = compose.runOnUiThread {
        val generation = ++generations
        compose.activity.setContent {
            // A new key per call: remember/rememberSaveable state must not leak from the previous show().
            key(generation) { Or2Theme {
                SessionPickerSheet(
                    caps, capsError, tmux, open,
                    openShell = { calls += "shell" }, openTmux = { calls += "tmux:$it" }, openHerdr = { calls += "herdr:$it" },
                    refresh = { calls += "refresh" }, dismiss = { calls += "dismiss" },
                    gate = gate, gateAction = { actions += it }, title = "Build box", udpBlocked = udpBlocked,
                )
            } }
        }
    }

    private fun pickerTab(index: Int) = compose.onNodeWithTag("picker-tab:$index").performClick()

    @Test
    fun herdrTmuxAndAPlainShellAreOneTapEachAndThereIsNoOpenTab() {
        val calls = mutableListOf<String>()
        show(calls)
        compose.onNodeWithTag("picker-title").assertTextEquals("Build box")
        compose.onNodeWithTag("picker-tab:2").assertDoesNotExist() // herdr and tmux only.
        // herdr is the first segment: the default session opens without a name, a named one by name, a stopped one not at all.
        compose.onNodeWithTag("herdr-open:old").assertIsNotEnabled()
        compose.onNodeWithTag("herdr-open:default").performClick()
        compose.onNodeWithTag("herdr-open:work").performClick()
        pickerTab(1)
        compose.onNodeWithText("3 windows · 1 attached").assertIsDisplayed()
        compose.onNodeWithTag("tmux-attach:build").performClick()
        compose.onNodeWithTag("host-shell").assertTextEquals("Shell").performClick() // A plain shell.
        compose.onNodeWithTag("host-refresh").performScrollTo().performClick()
        compose.runOnIdle { assertEquals(listOf("herdr:null", "herdr:work", "tmux:build", "shell", "refresh"), calls) }
    }

    @Test
    fun aSessionThatIsOpenInOr2IsMarkedOpen() {
        val calls = mutableListOf<String>()
        show(calls, open = OpenSessions(herdr = setOf(null), tmux = setOf("build")))
        compose.onNodeWithTag("open-mark:herdr:default", useUnmergedTree = true).assertIsDisplayed()
        compose.onNodeWithTag("open-mark:herdr:work", useUnmergedTree = true).assertDoesNotExist()
        compose.onNodeWithText("Running", useUnmergedTree = true).assertIsDisplayed() // work: running, not open.
        compose.onNodeWithTag("herdr-open:default").performClick() // The app switches to its terminal (TerminalActivations).
        pickerTab(1)
        compose.onNodeWithTag("open-mark:tmux:build", useUnmergedTree = true).assertIsDisplayed()
        compose.onNodeWithTag("open-mark:tmux:main", useUnmergedTree = true).assertDoesNotExist()
        compose.onNodeWithTag("tmux-attach:build").performClick()
        compose.runOnIdle { assertEquals(listOf("herdr:null", "tmux:build"), calls) }
    }

    @Test
    fun aBlockedUdpVerdictShowsOneMutedLineUnderTheTabs() {
        show(udpBlocked = true)
        compose.onNodeWithTag("picker-udp-blocked").assertIsDisplayed().assertTextEquals(UDP_BLOCKED_LINE)
        show(udpBlocked = false)
        compose.onNodeWithTag("picker-udp-blocked").assertDoesNotExist()
        // Before the host connects, the gate shows instead of anything about its terminals.
        show(udpBlocked = true, gate = PickerGate.Connecting("Build box", "Checking server…", spinning = true))
        compose.onNodeWithTag("picker-udp-blocked").assertDoesNotExist()
    }

    @Test
    fun newTmuxSessionNamesAreValidatedBeforeCreating() {
        val calls = mutableListOf<String>()
        show(calls)
        pickerTab(1)
        compose.onNodeWithTag("tmux-new").performScrollTo().assertIsNotEnabled()
        compose.onNodeWithTag("tmux-new-name").performScrollTo().performTextInput("a:b")
        compose.onNodeWithText("Remove backslash, colon and dot characters.").assertIsDisplayed()
        compose.onNodeWithTag("tmux-new").assertIsNotEnabled()
        compose.onNodeWithTag("tmux-new-name").performTextInput("x") // "a:bx" is still invalid.
        compose.onNodeWithTag("tmux-new").assertIsNotEnabled()
        show(calls)
        pickerTab(1)
        compose.onNodeWithTag("tmux-new-name").performScrollTo().performTextInput("my work")
        compose.onNodeWithTag("tmux-new").performScrollTo().assertIsEnabled().performClick()
        compose.runOnIdle { assertEquals(listOf("tmux:my work"), calls) }
    }

    @Test
    fun missingTmuxHerdrAndFailedListingsAreExplainedNotHidden() {
        show(caps = caps.copy(tmux = null, herdr = null))
        compose.onNodeWithTag("herdr-missing").assertIsDisplayed()
        pickerTab(1)
        compose.onNodeWithTag("tmux-missing").assertIsDisplayed()
        show(tmux = TmuxList.Failed("The connection has closed. Reconnect to continue."))
        pickerTab(1)
        compose.onNodeWithText("The connection has closed. Reconnect to continue.").assertIsDisplayed()
        show(caps = null, tmux = TmuxList.Loading)
        compose.onNodeWithText("Checking the host…", substring = true).assertIsDisplayed()
        show(capsError = "probe failed")
        compose.onNodeWithText("Could not query the host. Try Refresh.").assertIsDisplayed()
    }

    @Test
    fun beforeItsHostConnectsThePickerShowsProgressThenWhyNotWithOneAction() {
        val actions = mutableListOf<GateAction>()
        show(gate = PickerGate.Connecting("Build box", "Checking server…", spinning = true), actions = actions)
        compose.onNodeWithTag("session-picker").assertIsDisplayed()
        compose.onNodeWithTag("picker-progress").assertTextEquals("Checking server…")
        compose.onNodeWithTag("picker-spinner").assertIsDisplayed()
        compose.onNodeWithTag("picker-tab:0").assertDoesNotExist() // No lists until the host is connected.
        compose.onNodeWithTag("host-shell").assertDoesNotExist()
        show(gate = PickerGate.Stopped("Build box", "Authentication rejected.", failed = true, detail = "a.invalid:22 · refused",
            action = GateAction.RETRY, enabled = true), actions = actions)
        compose.onNodeWithTag("picker-reason").assertTextEquals("Authentication rejected.")
        compose.onNodeWithTag("picker-detail").assertIsDisplayed()
        compose.onNodeWithTag("picker-retry").assertTextEquals("Retry").performClick()
        show(gate = PickerGate.Stopped("Build box", "Not connected", false, null, GateAction.CONNECT, enabled = false), actions = actions)
        compose.onNodeWithTag("picker-retry").assertTextEquals("Connect").assertIsNotEnabled()
        compose.runOnIdle { assertEquals(listOf(GateAction.RETRY), actions) }
        // Connected: the gate is gone and the lists show in the same sheet.
        show(gate = null, actions = actions)
        compose.onNodeWithTag("picker-gate").assertDoesNotExist()
        compose.onNodeWithTag("herdr-open:work").assertIsDisplayed()
    }

    @Test
    fun aHostWithoutHerdrOpensOnTheTmuxSegment() {
        show(caps = caps.copy(herdr = null, herdrSessions = emptyList()))
        compose.onNodeWithTag("tmux-attach:main").assertIsDisplayed()
    }
}
