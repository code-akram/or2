package io.github.code_akram.or2.app

import androidx.activity.compose.setContent
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertTextEquals
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onAllNodesWithTag
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performSemanticsAction
import io.github.code_akram.or2.MainActivity
import io.github.code_akram.or2.connection.HostConnections
import io.github.code_akram.or2.connection.UiPort
import io.github.code_akram.or2.connection.UiTrust
import io.github.code_akram.or2.connection.uiHost
import io.github.code_akram.or2.data.KeyRecord
import io.github.code_akram.or2.ffi.AgentStatus
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.HerdrAgent
import io.github.code_akram.or2.ffi.HerdrState
import io.github.code_akram.or2.ffi.HerdrTab
import io.github.code_akram.or2.ffi.HerdrView
import io.github.code_akram.or2.ffi.HerdrWorkspace
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.SessionFailure
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.ffi.TmuxSession
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

/**
 * Home is the one place for hosts and their terminals, in the whole app (`Or2App`) over fakes (no network, Keystore or
 * database): a host card's header opens the session picker over Home, connecting the host first when it is not;
 * choosing a target opens the terminal, a session that is already open (`● Open`) switches to its terminal; the
 * host's terminals sit in its card, each with its `×`; Back from a terminal goes Home.
 */
class HomeSessionPickerDeviceTest {
    @get:Rule val compose = createAndroidComposeRule<MainActivity>()

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val key = KeyRecord("fixture-key", "Fixture key", "ssh-ed25519", "ssh-ed25519 AAAA", "SHA256:fixture", "", byteArrayOf(), byteArrayOf())
    private val host = uiHost(id = 7, label = "Alpha")
    private val port = UiPort()
    private val holder = HostConnections({ _, listener -> port.also { it.hostListener = listener } }, UiTrust(), worker = Dispatchers.Unconfined)

    /** Hosts the app asked to connect, one list per unlock. The unlock succeeds shortly and the connect starts. */
    private val connected = mutableListOf<List<Long>>()

    /** True from the tap until the connect has started, as while the real app's biometric prompt is up. */
    private var busy by mutableStateOf(false)

    @After
    fun stop() = scope.cancel()

    private fun show() = compose.runOnUiThread {
        compose.activity.setContent {
            key(Unit) {
                Or2App(
                    listOf(host), listOf(key), null, busy = busy, holder,
                    AppActions(
                        saveHost = { _, _ -> }, deleteHost = {}, generateKey = { _, _ -> }, importKey = { _, _, _ -> }, deleteKey = {},
                        connect = { list ->
                            connected += list.map { it.id }
                            busy = true
                            scope.launch {
                                delay(50) // The unlock: a frame or two, so Home sees busy come and go.
                                try {
                                    holder.connect(list, byteArrayOf(1))
                                } finally {
                                    busy = false
                                }
                            }
                        },
                        approve = { _, _ -> }, reject = {}, message = {},
                    ),
                )
            }
        }
    }

    /** The host's connection reports [state], as Rust would. */
    private fun report(state: HostState) = compose.runOnUiThread {
        port.native = state
        port.hostListener!!.onHostStateChanged(state)
    }

    private fun connectFirst() {
        compose.runOnUiThread { runBlocking { holder.connect(host, byteArrayOf(1)) } }
        report(HostState.Connected(0u))
        compose.waitUntil(5_000) { holder.host(7)?.state?.value is HostState.Connected }
    }

    private fun waitFor(tag: String, unmerged: Boolean = false) =
        compose.waitUntil(5_000) { compose.onAllNodesWithTag(tag, useUnmergedTree = unmerged).fetchSemanticsNodes().isNotEmpty() }

    /** Waits until the node tagged [tag] reads [text] (the connect runs off the test's thread). */
    private fun waitForText(tag: String, text: String) = compose.waitUntil(5_000) {
        compose.onAllNodesWithTag(tag).fetchSemanticsNodes().any { node ->
            node.config.getOrElse(SemanticsProperties.Text) { emptyList() }.joinToString("") { it.text } == text
        }
    }

    @Test
    fun theHeaderOpensThePickerOverHomeForAConnectedHostAndAChoiceOpensTheTerminal() {
        connectFirst()
        show()
        compose.onNodeWithTag("host:7").performClick()
        compose.onNodeWithTag("session-picker").assertIsDisplayed()
        compose.onNodeWithTag("picker-gate").assertDoesNotExist() // Connected: the lists at once.
        compose.onNodeWithTag("picker-title").assertTextEquals("Alpha") // Over Home, the sheet names its host.
        compose.onNodeWithTag("home-list").assertExists()
        compose.onNodeWithTag("picker-tab:2").assertDoesNotExist() // herdr and tmux only: no Open tab.
        waitFor("herdr-open:default")
        compose.onNodeWithTag("host-shell").performClick()
        waitFor("terminal-card")
        compose.onNodeWithTag("session-picker").assertDoesNotExist()
        compose.runOnIdle {
            assertEquals(listOf(TerminalTarget.Shell), port.sessions.map { it.first })
            assertEquals(emptyList<List<Long>>(), connected) // Connected already: nothing to unlock.
        }
    }

    @Test
    fun forAHostThatIsNotConnectedTheHeaderConnectsShowsProgressThenTheLists() {
        show()
        compose.onNodeWithTag("host:7").performClick()
        compose.runOnIdle { assertEquals(listOf(listOf(7L)), connected) } // The usual unlock and connect.
        compose.onNodeWithTag("session-picker").assertIsDisplayed()
        compose.onNodeWithTag("picker-progress").assertIsDisplayed() // At once: "Unlocking key…" while the unlock runs.
        waitForText("picker-progress", "Checking server…")
        compose.onNodeWithTag("picker-tab:0").assertDoesNotExist()
        report(HostState.Authenticating)
        compose.onNodeWithTag("picker-progress").assertTextEquals("Authenticating…")
        report(HostState.Connected(0u))
        // The same sheet, now with herdr and tmux.
        waitFor("herdr-open:default")
        compose.onNodeWithTag("picker-gate").assertDoesNotExist()
        compose.onNodeWithTag("picker-tab:1").performClick()
        waitFor("tmux-attach:main")
        compose.onNodeWithTag("tmux-attach:main").performClick()
        waitFor("terminal-card")
        compose.runOnIdle { assertEquals(listOf(TerminalTarget.Tmux("main")), port.sessions.map { it.first }) }
    }

    @Test
    fun aFailedConnectSaysWhyInTheSheetAndRetryConnectsAgain() {
        show()
        compose.onNodeWithTag("host:7").performClick()
        waitForText("picker-progress", "Checking server…")
        report(HostState.Closed(CloseReason.Failed(SessionFailure.AuthenticationRejected)))
        waitForText("picker-reason", "Authentication rejected. Check the username and public-key authorization.")
        compose.onNodeWithTag("picker-retry").assertTextEquals("Retry").performClick()
        compose.runOnIdle { assertEquals(listOf(listOf(7L), listOf(7L)), connected) }
        compose.onNodeWithTag("session-picker").assertIsDisplayed()
    }

    @Test
    fun dismissingThePickerLeavesHomeAsItWas() {
        connectFirst()
        show()
        compose.onNodeWithTag("host:7").performClick()
        compose.onNodeWithTag("session-picker").assertIsDisplayed()
        // The scrim's own dismiss action (a tap outside the sheet).
        compose.onNodeWithContentDescription("Close sheet").performSemanticsAction(SemanticsActions.OnClick)
        compose.onNodeWithTag("session-picker").assertDoesNotExist()
        compose.onNodeWithTag("home-list").assertIsDisplayed()
        compose.onNodeWithTag("host:7").assertIsDisplayed()
        compose.runOnIdle {
            assertTrue(port.sessions.isEmpty()) // Nothing was opened.
            assertTrue(holder.terminals.value.isEmpty())
        }
    }

    @Test
    fun anOpenSessionIsMarkedOpenAndChoosingItSwitchesToItsTerminal() {
        connectFirst()
        show()
        compose.onNodeWithTag("host:7").performClick()
        compose.onNodeWithTag("picker-tab:1").performClick()
        waitFor("tmux-attach:main")
        compose.onNodeWithTag("open-mark:tmux:main", useUnmergedTree = true).assertDoesNotExist()
        compose.onNodeWithTag("tmux-attach:main").performClick()
        waitFor("terminal-card")
        // Back from a terminal is Home, as its minimise disc is.
        compose.runOnUiThread { compose.activity.onBackPressedDispatcher.onBackPressed() }
        waitFor("home-list")
        // The terminal sits in its host's card; the picker marks its session Open, and choosing it switches back to it.
        compose.onNodeWithTag("host-terminals:7").assertIsDisplayed()
        compose.onNodeWithTag("host:7").performClick()
        compose.onNodeWithTag("picker-tab:1").performClick()
        // The mark sits inside its row, whose semantics merge into the row: only the unmerged tree has its tag.
        waitFor("open-mark:tmux:main", unmerged = true)
        compose.onNodeWithTag("tmux-attach:main").performClick()
        waitFor("terminal-card")
        compose.runOnIdle {
            assertEquals(1, port.sessions.size) // The same terminal, not a second one.
            assertEquals(1, holder.terminals.value.size)
        }
    }

    @Test
    fun theHerdrTabListsTheHostsLiveAgentsAndTappingOneOpensItsTerminalFocusedOnIt() {
        connectFirst()
        // The default session's watch (the host shows in the inbox) reports two agents, as Rust would.
        compose.waitUntil(5_000) { port.watchListeners.isNotEmpty() }
        val view = HerdrView(
            1uL, null, listOf(HerdrWorkspace("w1", 1u, "or2")), listOf(HerdrTab("w1:t1", "w1", 1u, "ui")), emptyList(),
            listOf(
                HerdrAgent("w1:p1", "w1:t1", "w1", null, null, "Claude Code", AgentStatus.WORKING, "~/code/or2", 1uL, "term_1"),
                HerdrAgent("w1:p2", "w1:t1", "w1", null, null, "Codex", AgentStatus.BLOCKED, "~/code/or2", 1uL, "term_2"),
            ),
        )
        compose.runOnUiThread { port.watchListeners.first().onHerdrStateChanged(HerdrState.Live(view)) }
        show()
        compose.onNodeWithTag("host:7").performClick()
        waitFor("herdr-agent:default:w1:p2")
        compose.onNodeWithTag("herdr-workspace:default:or2", useUnmergedTree = true).assertIsDisplayed()
        compose.onNodeWithTag("herdr-agent:default:w1:p2").performClick()
        waitFor("terminal-card")
        compose.onNodeWithTag("session-picker").assertDoesNotExist()
        compose.runOnIdle {
            // The Inbox tap's path: the pane is focused and the session's terminal opened on it.
            assertEquals(listOf<Pair<String?, String>>(null to "w1:p2"), port.focused)
            assertEquals(listOf<TerminalTarget>(TerminalTarget.Herdr(null, "w1:p2")), port.sessions.map { it.first })
        }
        // Back Home, Whole session shows that terminal as it is: no second client, no focus.
        compose.runOnUiThread { compose.activity.onBackPressedDispatcher.onBackPressed() }
        waitFor("home-list")
        compose.onNodeWithTag("host:7").performClick()
        waitFor("open-mark:herdr:default", unmerged = true)
        compose.onNodeWithTag("herdr-open:default").performClick()
        waitFor("terminal-card")
        compose.runOnIdle {
            assertEquals(1, port.sessions.size)
            assertEquals(1, port.focused.size)
        }
    }

    @Test
    fun theTmuxListIsReadAgainEachTimeThePickerOpens() {
        connectFirst()
        show()
        compose.onNodeWithTag("host:7").performClick()
        compose.onNodeWithTag("picker-tab:1").performClick()
        waitFor("tmux-attach:main")
        compose.onNodeWithTag("tmux-attach:fresh").assertDoesNotExist()
        compose.onNodeWithContentDescription("Close sheet").performSemanticsAction(SemanticsActions.OnClick)
        // A session made on the host meanwhile shows the next time the picker opens, without Refresh.
        compose.runOnUiThread { port.tmux = port.tmux + TmuxSession("fresh", 1u, 0u) }
        compose.onNodeWithTag("host:7").performClick()
        compose.onNodeWithTag("picker-tab:1").performClick()
        waitFor("tmux-attach:fresh")
    }

    @Test
    fun aHostsTerminalsSitInItsCardAndTheirCrossClosesThem() {
        connectFirst()
        val (tmux, shell) = compose.runOnIdle {
            holder.openTerminal(holder.host(7)!!, TerminalTarget.Tmux("main")) to holder.openTerminal(holder.host(7)!!, TerminalTarget.Shell)
        }
        show()
        compose.onNodeWithTag("session-card:${tmux.id}").assertIsDisplayed()
        compose.onNodeWithTag("session-card:${shell.id}").assertIsDisplayed()
        // tmux: only or2's view ends (the session runs on), in one tap.
        compose.onNodeWithTag("session-close:${tmux.id}").performClick()
        compose.runOnIdle { assertEquals(listOf(shell), holder.terminals.value) }
        // A shell ends with its programs: asked first.
        compose.onNodeWithTag("session-close:${shell.id}").performClick()
        compose.onNodeWithTag("close-shell-confirm").assertIsDisplayed().performClick()
        compose.runOnIdle { assertTrue(holder.terminals.value.isEmpty()) }
        compose.onNodeWithTag("host-terminals:7").assertDoesNotExist()
    }
}
