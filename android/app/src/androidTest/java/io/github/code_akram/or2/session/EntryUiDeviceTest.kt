package io.github.code_akram.or2.session

import io.github.code_akram.or2.assertTouchTargetAtLeast
import android.view.View
import android.view.ViewGroup
import androidx.activity.compose.setContent
import androidx.compose.material3.Text
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createAndroidComposeRule
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
        compose.onNodeWithText("Close session").assertIsDisplayed()
        compose.onNodeWithContentDescription("Terminal").assertDoesNotExist()
        compose.onNodeWithContentDescription("Keyboard").assertDoesNotExist()
        compose.onNodeWithTag("key:Esc").assertDoesNotExist()
        compose.onNodeWithTag("terminal-card").assertDoesNotExist()
        compose.runOnIdle { assertEquals(0, session.frameTakes) }
        compose.onNodeWithText("Close session").performClick()
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
        fun View.terminal(): TerminalView? {
            if (this is TerminalView) return this
            if (this is ViewGroup) for (index in 0 until childCount) getChildAt(index).terminal()?.let { return it }
            return null
        }
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
        compose.onNodeWithText("Fixture: shell").assertIsDisplayed()
        compose.onNodeWithText("SSH").assertIsDisplayed() // The transport badge.
        // The header's round buttons are drawn small (18 dp) but are full 48 dp touch targets.
        listOf("terminal-back", "terminal-panes").forEach {
            compose.onNodeWithTag(it).assertTouchTargetAtLeast(48)
        }
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
        // Disconnect from the panes sheet: the final frame stays until the session is closed.
        compose.onNodeWithTag("terminal-panes").performClick()
        compose.onNodeWithTag("terminal-disconnect").performClick()
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
}
