package io.github.code_akram.or2.session

import android.view.View
import android.view.ViewGroup
import androidx.activity.compose.setContent
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
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
import androidx.compose.ui.test.performTextInput
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import io.github.code_akram.or2.app.AppScaffold
import io.github.code_akram.or2.app.Destination
import io.github.code_akram.or2.connection.HostConnections
import io.github.code_akram.or2.connection.UiPort
import io.github.code_akram.or2.connection.UiSession
import io.github.code_akram.or2.connection.UiTrust
import io.github.code_akram.or2.connection.uiHost
import io.github.code_akram.or2.MainActivity
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.HostEndpoint
import io.github.code_akram.or2.data.HostRecord
import io.github.code_akram.or2.data.KeyRecord
import io.github.code_akram.or2.ffi.*
import io.github.code_akram.or2.hosts.HostsScreen
import io.github.code_akram.or2.terminal.TerminalView
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

/** Render entry/trust states without connecting to any host or modifying the production database. */
class EntryUiDeviceTest {
    @get:Rule val compose = createAndroidComposeRule<MainActivity>()

    private val fixtureKey = KeyRecord("fixture-key", "Fixture key", "test", "public", "fingerprint", "", byteArrayOf(), byteArrayOf())

    private fun keyChoice(key: KeyRecord) = compose.onNodeWithTag("host-key:${key.id}")
        .assert(SemanticsMatcher.expectValue(SemanticsProperties.Role, Role.RadioButton))

    private fun fillHostFields() {
        compose.onNodeWithText("Label").performScrollTo().performTextInput("Fixture")
        compose.onNodeWithText("Hostname").performScrollTo().performTextInput("fixture.invalid")
        compose.onNodeWithText("Username").performScrollTo().performTextInput("fixture-user")
    }

    @Test
    fun newHostPreselectsTheOnlyKeyButEditingDoesNotChangeAnEmptyReference() {
        var saved: Host? = null
        val previous = Host(HostRecord(7, "Existing fixture", "fixture-user", null), listOf(HostEndpoint("fixture.invalid", 22)))
        compose.runOnUiThread {
            compose.activity.setContent {
                MaterialTheme { HostsScreen(listOf(previous), listOf(fixtureKey), false, { host, _ -> saved = host }, {}, {}) }
            }
        }
        compose.onNodeWithText("Add host").performClick()
        keyChoice(fixtureKey).performScrollTo().assertIsSelected()
        compose.onNodeWithText("Choose a key").assertDoesNotExist()
        fillHostFields()
        compose.onNodeWithText("Save").assertIsEnabled().performClick()
        compose.runOnIdle { assertEquals(fixtureKey.id, saved!!.keyId) }
        compose.onNodeWithText("Edit").performClick()
        keyChoice(fixtureKey).performScrollTo().assertIsNotSelected()
        compose.onNodeWithText("Choose a key").performScrollTo().assertIsDisplayed()
        compose.onNodeWithText("Save").assertIsNotEnabled()
        compose.onNodeWithText("Cancel").performClick()
    }

    @Test
    fun multipleKeysNeedAnExplicitChoiceAndExposeTheSelectedRadioState() {
        var saved: Host? = null
        // Duplicate labels must not make the test target a different choice.
        val second = fixtureKey.copy(id = "second-key")
        compose.runOnUiThread {
            compose.activity.setContent {
                MaterialTheme { HostsScreen(emptyList(), listOf(fixtureKey, second), false, { host, _ -> saved = host }, {}, {}) }
            }
        }
        compose.onNodeWithText("Add host").performClick()
        keyChoice(fixtureKey).performScrollTo().assertIsNotSelected()
        keyChoice(second).performScrollTo().assertIsNotSelected()
        fillHostFields()
        compose.onNodeWithText("Choose a key").performScrollTo().assertIsDisplayed()
        compose.onNodeWithText("Save").assertIsNotEnabled()
        keyChoice(second).performScrollTo().performClick().assertIsSelected()
        keyChoice(fixtureKey).assertIsNotSelected()
        compose.onNodeWithText("Choose a key").assertDoesNotExist()
        compose.onNodeWithText("Save").assertIsEnabled().performClick()
        compose.runOnIdle { assertEquals(second.id, saved!!.keyId) }
    }

    @Test
    fun firstUseTrustIsExplicitAndRejectWorks() {
        var decision = ""
        val prompt = HostState.AwaitingHostKeyDecision(PublicKeyInfo("test", "public", "presented-fingerprint", ""), emptyList())
        compose.runOnUiThread {
            compose.activity.setContent { MaterialTheme { HostTrustDialog(prompt, false, { decision = "approve" }, { decision = "reject" }) } }
        }
        compose.onNodeWithText("Trust this host key?").assertIsDisplayed()
        compose.onNodeWithText("presented-fingerprint").assertIsDisplayed()
        assertEquals("", decision)
        compose.onNodeWithText("Reject").performClick()
        assertEquals("reject", decision)
    }

    @Test
    fun changedTrustShowsAllPreviousFingerprintsAndReplacementLabel() {
        var decision = ""
        val old = listOf(PublicKeyInfo("old-a", "a", "previous-a", ""), PublicKeyInfo("old-b", "b", "previous-b", ""))
        val prompt = HostState.AwaitingHostKeyDecision(PublicKeyInfo("new", "c", "presented-fingerprint", ""), old)
        compose.runOnUiThread {
            compose.activity.setContent { MaterialTheme { HostTrustDialog(prompt, false, { decision = "approve" }, { decision = "reject" }) } }
        }
        compose.onNodeWithText("WARNING: HOST KEY CHANGED").assertIsDisplayed()
        compose.onNodeWithText("old-a\nprevious-a").assertIsDisplayed()
        compose.onNodeWithText("old-b\nprevious-b").assertIsDisplayed()
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
                AppScaffold(Destination.Inbox, terminalVisible = false, fullScreen = true, selectTab = {}) {
                    SessionScreen(holder, terminal, listOf(terminal), back = {}, select = {})
                }
            }
        }
        compose.onNodeWithText("or2").assertIsDisplayed()
        compose.onNodeWithText("• Inbox").assertIsDisplayed()
        compose.onNodeWithText("Hosts").assertIsDisplayed()
        compose.onNodeWithText("Keys").assertIsDisplayed()
        compose.onNodeWithText("The server refused a terminal or shell.", substring = true).assertIsDisplayed()
        compose.onNodeWithText("Close session").assertIsDisplayed()
        compose.onNodeWithContentDescription("Terminal").assertDoesNotExist()
        compose.onNodeWithText("Keyboard").assertDoesNotExist()
        compose.onNodeWithText("Esc").assertDoesNotExist()
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
                AppScaffold(Destination.Inbox, terminalVisible = false, fullScreen = true, selectTab = {}) {
                    SessionScreen(holder, shell, listOf(shell, tmux), back = {}, select = { selected = it.id })
                }
            }
        }
        compose.onNodeWithTag("terminal-switcher").assertIsDisplayed()
        compose.onNodeWithText("• Fixture · shell").assertIsDisplayed()
        compose.onNodeWithText("Fixture · tmux work").performClick()
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
                val connected = terminal?.hasConnected?.collectAsState()?.value == true
                AppScaffold(Destination.Inbox, terminalVisible = screen == "terminal" && connected, fullScreen = screen == "terminal", selectTab = {}) {
                    if (screen == "terminal") SessionScreen(holder, terminal, terminals, back = { screen = "inbox" }, select = {})
                    else Text("Inbox fixture")
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
        compose.onNodeWithText("or2").assertDoesNotExist()
        compose.onNodeWithText("Hosts").assertDoesNotExist()
        compose.onNodeWithText("Keys").assertDoesNotExist()
        compose.runOnIdle {
            val insets = ViewCompat.getRootWindowInsets(view)!!.getInsets(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout())
            assertEquals(compose.activity.window.decorView.width - insets.left - insets.right, view.width)
        }
        val retained = holder.terminals.value.single()
        // Back leaves the terminal running: it is neither disconnected nor destroyed.
        compose.onNodeWithTag("terminal-back").performClick()
        compose.onNodeWithText("or2").assertIsDisplayed()
        compose.onNodeWithText("• Inbox").assertIsDisplayed()
        compose.onNodeWithText("Inbox fixture").assertIsDisplayed()
        compose.runOnIdle {
            assertSame(retained, holder.terminals.value.single())
            assertEquals(SessionState.Connected, retained.state.value)
            assertFalse(session.destroyed)
        }
        compose.runOnUiThread { screen = "terminal" }
        view = awaitTerminal()
        compose.onNodeWithText("or2").assertDoesNotExist()
        compose.onNodeWithText("Disconnect").performClick()
        compose.onNodeWithText("Close").assertIsDisplayed()
        compose.onNodeWithText("Disconnected", substring = true).assertIsDisplayed()
        compose.onNodeWithText("or2").assertDoesNotExist()
        compose.onNodeWithText("Keys").assertDoesNotExist()
        compose.runOnIdle {
            assertSame(view, compose.activity.window.decorView.terminal())
            assertEquals("LR", view.grid.rows.single().cells.joinToString("") { it.text })
            assertFalse(session.destroyed)
        }
        compose.onNodeWithText("Close").performClick()
        compose.onNodeWithText("or2").assertIsDisplayed()
        compose.onNodeWithText("Keys").assertIsDisplayed()
        compose.onNodeWithText("Inbox fixture").assertIsDisplayed()
        compose.waitUntil(5_000) {
            var closed = false
            compose.runOnUiThread { closed = session.destroyed }
            closed
        }
        compose.runOnIdle {
            assertEquals(1, session.closes)
            assertTrue(holder.terminals.value.isEmpty())
        }
    }
}
