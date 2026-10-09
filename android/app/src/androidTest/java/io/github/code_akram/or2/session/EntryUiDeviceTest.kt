package io.github.code_akram.or2.session

import io.github.code_akram.or2.assertTouchTargetAtLeast
import android.content.ClipboardManager
import android.view.View
import android.view.ViewGroup
import androidx.activity.compose.setContent
import androidx.compose.material3.Text
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertTextEquals
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onAllNodesWithTag
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import io.github.code_akram.or2.MainActivity
import io.github.code_akram.or2.app.AppScaffold
import io.github.code_akram.or2.connection.HostConnections
import io.github.code_akram.or2.connection.UiPort
import io.github.code_akram.or2.connection.UiSession
import io.github.code_akram.or2.connection.UiTrust
import io.github.code_akram.or2.connection.uiHost
import io.github.code_akram.or2.ffi.*
import io.github.code_akram.or2.terminal.TerminalView
import io.github.code_akram.or2.ui.Or2Theme
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

/** Render trust and terminal entry states without connecting to any host or modifying the production database. */
class EntryUiDeviceTest {
    @get:Rule val compose = createAndroidComposeRule<MainActivity>()

    @Test
    fun firstUseTrustIsExplicitAndRejectWorks() {
        var decision = ""
        val prompt = HostState.AwaitingHostKeyDecision(PublicKeyInfo("test", "public", "presented-fingerprint", ""), emptyList())
        compose.runOnUiThread {
            compose.activity.setContent { Or2Theme { HostTrustDialog(prompt, false, { decision = "approve" }, { decision = "reject" }) } }
        }
        compose.onNodeWithText("Trust this host key?").assertIsDisplayed()
        compose.onNodeWithText("presented-fingerprint").assertIsDisplayed()
        assertEquals("", decision)
        compose.onNodeWithTag("hostkey-reject").performClick()
        assertEquals("reject", decision)
    }

    @Test
    fun changedTrustShowsAllPreviousFingerprintsAndReplacementLabel() {
        var decision = ""
        val old = listOf(PublicKeyInfo("old-a", "a", "previous-a", ""), PublicKeyInfo("old-b", "b", "previous-b", ""))
        val prompt = HostState.AwaitingHostKeyDecision(PublicKeyInfo("new", "c", "presented-fingerprint", ""), old)
        compose.runOnUiThread {
            compose.activity.setContent {
                Or2Theme { HostTrustDialog(prompt, false, { decision = "approve" }, { decision = "reject" }, hostLabel = "Fixture host") }
            }
        }
        compose.onNodeWithText("WARNING: HOST KEY CHANGED").assertIsDisplayed()
        compose.onNodeWithText("Host: Fixture host").assertIsDisplayed()
        // Algorithm and fingerprint are separate lines now; both previous keys are listed.
        compose.onNodeWithText("old-a").assertIsDisplayed()
        compose.onNodeWithText("previous-a").assertIsDisplayed()
        compose.onNodeWithText("old-b").assertIsDisplayed()
        compose.onNodeWithText("previous-b").assertIsDisplayed()
        compose.onNodeWithText("presented-fingerprint").assertIsDisplayed()
        compose.onNodeWithText("Replace trust and connect").performClick()
        assertEquals("approve", decision)
    }

    private fun terminalHolder(port: UiPort): HostConnections {
        val holder = HostConnections({ _, listener -> port.also { it.hostListener = listener } }, UiTrust(), worker = Dispatchers.Unconfined)
        compose.runOnUiThread {
            runBlocking { holder.connect(uiHost(), byteArrayOf(1)) }
            port.native = HostState.Connected(0u)
        }
        return holder
    }

    @Test
    fun terminalClosedBeforeConnectedHasNoTerminalOrKeyboardControls() {
        val closed = SessionState.Closed(CloseReason.Failed(SessionFailure.ShellRejected))
        val session = UiSession(closed)
        val holder = terminalHolder(UiPort(closed) { session })
        compose.runOnUiThread {
            val terminal = holder.openTerminal(holder.host(1)!!, TerminalTarget.Shell)
            compose.activity.setContent {
                AppScaffold(fullScreen = false) {
                    SessionScreen(holder, terminal, listOf(terminal), minimise = {}, select = {})
                }
            }
        }
        compose.onNodeWithText("Fixture").assertIsDisplayed()
        compose.onNodeWithTag("terminal-status").assertIsDisplayed()
        compose.onNodeWithText("The server refused a terminal or shell.", substring = true).assertIsDisplayed()
        compose.onNodeWithText("Close session").assertDoesNotExist() // The terminal's own × closes it.
        compose.onNodeWithContentDescription("Terminal").assertDoesNotExist()
        compose.onNodeWithContentDescription("Keyboard").assertDoesNotExist()
        compose.onNodeWithTag("key:Esc").assertDoesNotExist()
        compose.onNodeWithTag("terminal-card").assertDoesNotExist()
        compose.runOnIdle { assertEquals(0, session.frameTakes) }
        // Closed already: nothing to ask, one tap takes it away.
        compose.onNodeWithTag("terminal-row-close:1").performClick()
        compose.runOnIdle { assertTrue(holder.terminals.value.isEmpty()) }
    }

    @Test
    fun theSwitcherListsOpenTerminalsAndSelectingOneSwitchesWithoutDisconnecting() {
        val port = UiPort(SessionState.Connecting)
        val holder = terminalHolder(port)
        var selected: Long? = null
        compose.runOnUiThread {
            val shell = holder.openTerminal(holder.host(1)!!, TerminalTarget.Shell)
            val tmux = holder.openTerminal(holder.host(1)!!, TerminalTarget.Tmux("work"))
            compose.activity.setContent {
                AppScaffold(fullScreen = false) {
                    SessionScreen(holder, shell, listOf(shell, tmux), minimise = {}, select = { selected = it.id })
                }
            }
        }
        compose.onNodeWithTag("terminal-switcher").assertIsDisplayed()
        compose.onNodeWithTag("terminal-tab:1").assertIsDisplayed()
        compose.onNodeWithText("Current").assertIsDisplayed() // The shown terminal is marked.
        compose.onNodeWithText("tmux work").assertIsDisplayed()
        compose.onNodeWithTag("terminal-tab:2").performClick()
        compose.runOnIdle {
            assertEquals(2L, selected)
            assertTrue(port.sessions.none { it.second.destroyed })
        }
    }

    @Test
    fun sessionScreenRetainsTerminalAndLastGridThroughClosedUntilClose() {
        val session = UiSession()
        val holder = terminalHolder(UiPort { session })
        var screen by mutableStateOf("terminal")
        compose.runOnUiThread {
            holder.openTerminal(holder.host(1)!!, TerminalTarget.Shell)
            compose.activity.setContent {
                val terminals by holder.terminals.collectAsState()
                val terminal = terminals.firstOrNull()
                AppScaffold(fullScreen = screen == "terminal") {
                    if (screen == "terminal") SessionScreen(holder, terminal, terminals, minimise = { screen = "home" }, select = {})
                    else Text("Home fixture")
                }
            }
        }
        fun awaitTerminal(): TerminalView {
            var view: TerminalView? = null
            compose.waitUntil(5_000) {
                var ready = false
                compose.runOnUiThread {
                    view = compose.activity.window.decorView.terminal()
                    ready = view?.grid?.hasGrid == true
                }
                ready
            }
            return view!!
        }
        var view = awaitTerminal()
        compose.onNodeWithTag("terminal-title").assertIsDisplayed()
        compose.onNodeWithText("Fixture · shell").assertIsDisplayed()
        compose.onNodeWithText("SSH").assertIsDisplayed() // The transport badge.
        // The header's round discs are drawn small (16 dp, a pair 10 dp apart) but are full 48 dp touch targets.
        listOf("terminal-back", "terminal-panes").forEach {
            compose.onNodeWithTag(it).assertTouchTargetAtLeast(48)
        }
        // Their boxes meet halfway between the discs: no gap and no overlap, so a tap between them goes to the nearer one.
        val back = compose.onNodeWithTag("terminal-back").fetchSemanticsNode().boundsInRoot
        val panes = compose.onNodeWithTag("terminal-panes").fetchSemanticsNode().boundsInRoot
        assertEquals("header buttons $back / $panes", back.right, panes.left, 1f)
        compose.runOnIdle {
            val insets = ViewCompat.getRootWindowInsets(view)!!.getInsets(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout())
            assertEquals(compose.activity.window.decorView.width - insets.left - insets.right, view.width)
        }
        val retained = holder.terminals.value.single()
        // Minimising leaves the terminal running: it is neither disconnected nor destroyed.
        compose.onNodeWithTag("terminal-back").performClick()
        compose.onNodeWithText("Home fixture").assertIsDisplayed()
        compose.runOnIdle {
            assertSame(retained, holder.terminals.value.single())
            assertEquals(SessionState.Connected, retained.state.value)
            assertFalse(session.destroyed)
        }
        compose.runOnUiThread { screen = "terminal" }
        view = awaitTerminal()
        // A session that closes by itself (here the server's end): the final frame stays until the user closes it.
        compose.runOnUiThread { session.listener!!.onStateChanged(SessionState.Closed(CloseReason.Disconnected)) }
        compose.onNodeWithTag("terminal-close").assertIsDisplayed()
        compose.onNodeWithText("Disconnected", substring = true).assertIsDisplayed()
        compose.runOnIdle {
            assertSame(view, compose.activity.window.decorView.terminal())
            assertEquals("LR", view.grid.rows.single().cells.joinToString("") { it.text })
            assertFalse(session.destroyed)
        }
        compose.onNodeWithTag("terminal-close").performClick()
        compose.onNodeWithText("Home fixture").assertIsDisplayed()
        compose.waitUntil(5_000) {
            var closed = false
            compose.runOnUiThread { closed = session.destroyed }
            closed
        }
        compose.runOnIdle {
            assertEquals(1, session.closes)
            assertTrue(holder.terminals.value.isEmpty())
        }
        assertNotNull(view)
    }

    /** The Terminals sheet over [targets] opened on one host, the first on screen; Home is a text fixture. */
    private fun showTerminals(holder: HostConnections, vararg targets: TerminalTarget): MutableList<Long> {
        val selected = mutableListOf<Long>()
        var screen by mutableStateOf("terminal")
        compose.runOnUiThread {
            targets.forEach { holder.openTerminal(holder.host(1)!!, it) }
            compose.activity.setContent {
                val terminals by holder.terminals.collectAsState()
                AppScaffold(fullScreen = screen == "terminal") {
                    if (screen == "terminal") {
                        SessionScreen(holder, terminals.firstOrNull(), terminals, minimise = { screen = "home" }, select = { selected += it.id })
                    } else {
                        Text("Home fixture")
                    }
                }
            }
        }
        compose.waitUntil(5_000) { compose.onAllNodesWithTag("terminal-panes").fetchSemanticsNodes().isNotEmpty() }
        compose.onNodeWithTag("terminal-panes").performClick()
        compose.onNodeWithTag("terminals-sheet").assertIsDisplayed()
        return selected
    }

    @Test
    fun theTerminalsSheetGroupsTerminalsByHostAndItsCrossClosesOneInOneTap() {
        val sessions = mutableListOf<UiSession>()
        val holder = terminalHolder(UiPort { UiSession().also(sessions::add) })
        val selected = showTerminals(holder, TerminalTarget.Shell, TerminalTarget.Tmux("work"))
        // One header for the host; the shown terminal is marked; no separate "Close session" row.
        compose.onNodeWithText("FIXTURE").assertIsDisplayed()
        compose.onNodeWithText("Current").assertIsDisplayed()
        compose.onNodeWithTag("terminal-close-session").assertDoesNotExist()
        compose.onNodeWithTag("terminal-tab:2").performClick()
        compose.runOnIdle { assertEquals(listOf(2L), selected) }
        // tmux: only or2's view ends (the session runs on), in one tap, and the terminal on screen stays.
        compose.onNodeWithTag("terminal-panes").performClick()
        compose.onNodeWithTag("terminal-row-close:2").performClick()
        compose.onNodeWithText("Close shell?").assertDoesNotExist()
        compose.runOnIdle {
            assertEquals(listOf(1L), holder.terminals.value.map { it.id })
            assertTrue(sessions[1].destroyed)
        }
        compose.onNodeWithText("Home fixture").assertDoesNotExist()
    }

    @Test
    fun closingTheShellOnScreenAsksFirstThenEndsItAndReturnsHome() {
        val session = UiSession()
        val holder = terminalHolder(UiPort { session })
        showTerminals(holder, TerminalTarget.Shell)
        compose.onNodeWithTag("terminal-row-close:1").performClick()
        // A shell ends with its programs: asked first, and Cancel keeps it.
        compose.onNodeWithText("Close shell?").assertIsDisplayed()
        compose.onNodeWithText("Cancel").performClick()
        compose.runOnIdle { assertEquals(1, holder.terminals.value.size) }
        compose.onNodeWithTag("terminal-panes").performClick()
        compose.onNodeWithTag("terminal-row-close:1").performClick()
        compose.onNodeWithTag("close-shell-confirm").performClick()
        compose.onNodeWithText("Home fixture").assertIsDisplayed()
        compose.onNodeWithTag("terminal-close").assertDoesNotExist()
        compose.waitUntil(5_000) {
            var closed = false
            compose.runOnUiThread { closed = session.destroyed }
            closed
        }
        compose.runOnIdle {
            assertTrue(holder.terminals.value.isEmpty()) // Dismissed, not left behind as a closed thumbnail.
            assertEquals(1, session.closes)
        }
    }

    @Test
    fun copyScreenPutsTheVisibleTextOnTheClipboardAndTheShortcutsRowOpensTheSheet() {
        val holder = terminalHolder(UiPort { UiSession() })
        showTerminals(holder, TerminalTarget.Tmux("work"))
        compose.waitUntil(5_000) {
            var ready = false
            compose.runOnUiThread { ready = compose.activity.window.decorView.terminal()?.grid?.hasGrid == true }
            ready
        }
        compose.onNodeWithTag("terminals-copy-screen").performClick()
        compose.onNodeWithTag("terminals-sheet").assertDoesNotExist()
        compose.runOnIdle {
            val clip = compose.activity.getSystemService(ClipboardManager::class.java).primaryClip
            assertEquals("LR", clip?.getItemAt(0)?.text?.toString()) // The fixture's one row.
        }
        compose.onNodeWithTag("terminal-panes").performClick()
        compose.onNodeWithTag("terminals-shortcuts").performClick()
        compose.onNodeWithTag("shortcuts-sheet").assertIsDisplayed()
    }

    @Test
    fun historyReadsTheTargetsTextIntoAFullHeightSheetAndCopiesItAll() {
        val port = UiPort { UiSession() }
        port.history = HistoryText((1..120).joinToString("\n") { "history line $it" } + "\n\u001b[0m\n", true)
        val holder = terminalHolder(port)
        showTerminals(holder, TerminalTarget.Tmux("work"))
        compose.onNodeWithTag("terminals-history").performClick()
        compose.onNodeWithTag("terminals-sheet").assertDoesNotExist()
        compose.waitUntil(5_000) { compose.onAllNodesWithTag("history-text").fetchSemanticsNodes().isNotEmpty() }
        compose.onNodeWithTag("history-sheet").assertIsDisplayed()
        compose.onNodeWithTag("history-count").assertTextEquals("121 lines · older lines not shown")
        // Opened at the bottom: the newest line shows.
        compose.onNodeWithText("history line 120", substring = true).assertIsDisplayed()
        compose.onNodeWithTag("history-copy-all").performClick()
        compose.runOnIdle {
            val clip = compose.activity.getSystemService(ClipboardManager::class.java).primaryClip
            val text = clip?.getItemAt(0)?.text?.toString()
            assertEquals("history line 1", text?.lines()?.first())
            assertEquals("[0m", text?.lines()?.last()) // The escape itself was removed.
        }
    }

    @Test
    fun aShellOffersNoHistory() {
        val holder = terminalHolder(UiPort { UiSession() })
        showTerminals(holder, TerminalTarget.Shell)
        compose.onNodeWithTag("terminals-copy-screen").assertIsDisplayed()
        compose.onNodeWithTag("terminals-history").assertDoesNotExist()
    }
}

private fun View.terminal(): TerminalView? {
    if (this is TerminalView) return this
    if (this is ViewGroup) for (index in 0 until childCount) getChildAt(index).terminal()?.let { return it }
    return null
}
