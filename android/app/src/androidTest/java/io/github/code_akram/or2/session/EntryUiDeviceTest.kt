package io.github.code_akram.or2.session

import android.view.View
import android.view.ViewGroup
import androidx.activity.compose.setContent
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
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
import io.github.code_akram.or2.AppScaffold
import io.github.code_akram.or2.MainActivity
import io.github.code_akram.or2.data.HostRecord
import io.github.code_akram.or2.data.KeyRecord
import io.github.code_akram.or2.data.TrustStore
import io.github.code_akram.or2.ffi.*
import io.github.code_akram.or2.hosts.HostsScreen
import io.github.code_akram.or2.terminal.TerminalView
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertSame
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
        var saved: HostRecord? = null
        val previous = HostRecord(7, "Existing fixture", "fixture.invalid", 22, "fixture-user", null)
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
        var saved: HostRecord? = null
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
        val prompt = SessionState.AwaitingHostKeyDecision(PublicKeyInfo("test", "public", "presented-fingerprint", ""), emptyList())
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
        val prompt = SessionState.AwaitingHostKeyDecision(PublicKeyInfo("new", "c", "presented-fingerprint", ""), old)
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

    @Test
    fun authenticationRejectedBeforeConnectedHasNoTerminalOrKeyboardControls() {
        val closed = SessionState.Closed(CloseReason.Failed(SessionFailure.AuthenticationRejected))
        var frameTakes = 0
        val session = object : SessionInterface, AutoCloseable {
            override fun takeFrame(): TerminalFrame? { frameTakes++; return null }
            override fun requestFullFrame() = Unit
            override fun disconnect() = Unit
            override fun close() = Unit
            override fun resize(columns: UShort, rows: UShort) = Unit
            override fun sendText(text: String) = Unit
            override fun sendKey(input: KeyInput) = Unit
            override fun scroll(scroll: ViewportScroll) = Unit
            override fun state() = closed
            override fun approveHostKey(fingerprint: String) = Unit
            override fun rejectHostKey() = Unit
        }
        val store = object : TrustStore {
            override suspend fun trustedKeys(hostId: Long) = emptyList<String>()
            override suspend fun replaceTrust(host: HostRecord, presented: PublicKeyInfo) = Unit
        }
        val holder = SessionHolder(SessionConnector { _, listener ->
            listener.onStateChanged(SessionState.Authenticating)
            listener.onStateChanged(closed)
            session
        }, store, worker = Dispatchers.Unconfined)
        compose.runOnUiThread {
            runBlocking { holder.connect(HostRecord(1, "Fixture", "fixture.invalid", 22, "fixture", null), byteArrayOf(1)) }
            compose.activity.setContent { AppScaffold(holder, "Session", {}) { SessionScreen(holder, false, { _, _ -> }, {}) } }
        }
        compose.onNodeWithText("or2").assertIsDisplayed()
        compose.onNodeWithText("Hosts").assertIsDisplayed()
        compose.onNodeWithText("Keys").assertIsDisplayed()
        compose.onNodeWithText("Authentication rejected. Check the username and public-key authorization.").assertIsDisplayed()
        compose.onNodeWithText("Close session").assertIsDisplayed()
        compose.onNodeWithContentDescription("Terminal").assertDoesNotExist()
        compose.onNodeWithText("Keyboard").assertDoesNotExist()
        compose.onNodeWithText("Esc").assertDoesNotExist()
        compose.runOnIdle { assertEquals(0, frameTakes) }
        compose.onNodeWithText("Close session").performClick()
    }

    @Test
    fun sessionScreenRetainsTerminalAndLastGridThroughClosedUntilDismiss() {
        fun View.terminal(): TerminalView? {
            if (this is TerminalView) return this
            if (this is ViewGroup) for (index in 0 until childCount) getChildAt(index).terminal()?.let { return it }
            return null
        }
        lateinit var listener: SessionListener
        var destroyed = false
        var closes = 0
        var pending: TerminalFrame? = null
        val frame = TerminalFrame(1uL, 2u, 1u, true,
            listOf(CellStyle(0xffffffu, 0u, null, Underline.NONE, false, false, false, false, false)),
            listOf(TerminalRow(0u, false, listOf(TerminalCell("L", CellWidth.NARROW, 0u), TerminalCell("R", CellWidth.NARROW, 0u)))),
            null, 0u, Scrollback(1uL, 0uL))
        val session = object : SessionInterface, AutoCloseable {
            override fun takeFrame(): TerminalFrame? {
                check(!destroyed)
                return pending.also { pending = null }
            }
            override fun requestFullFrame() { check(!destroyed); pending = frame; listener.onFrameReady() }
            override fun disconnect() { check(!destroyed); listener.onStateChanged(SessionState.Closed(CloseReason.Disconnected)) }
            override fun close() { destroyed = true; closes++ }
            override fun resize(columns: UShort, rows: UShort) = Unit
            override fun sendText(text: String) = Unit
            override fun sendKey(input: KeyInput) = Unit
            override fun scroll(scroll: ViewportScroll) = Unit
            override fun state() = SessionState.Connected
            override fun approveHostKey(fingerprint: String) = Unit
            override fun rejectHostKey() = Unit
        }
        val store = object : TrustStore {
            override suspend fun trustedKeys(hostId: Long) = emptyList<String>()
            override suspend fun replaceTrust(host: HostRecord, presented: PublicKeyInfo) = Unit
        }
        val holder = SessionHolder(SessionConnector { _, callback ->
            listener = callback
            listener.onStateChanged(SessionState.Connected)
            session
        }, store, worker = Dispatchers.Unconfined)
        var tab by mutableStateOf("Session")
        compose.runOnUiThread {
            runBlocking { holder.connect(HostRecord(1, "Fixture", "fixture.invalid", 22, "fixture", null), byteArrayOf(1)) }
            compose.activity.setContent {
                AppScaffold(holder, tab, { tab = it }) {
                    if (tab == "Session") SessionScreen(holder, false, { _, _ -> }, {}) else Text("Hosts fixture")
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
        compose.onNodeWithText("• Session").assertDoesNotExist()
        compose.runOnIdle {
            val insets = ViewCompat.getRootWindowInsets(view)!!.getInsets(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout())
            assertEquals(compose.activity.window.decorView.width - insets.left - insets.right, view.width)
        }
        val retained = holder.active.value
        compose.runOnUiThread { compose.activity.onBackPressedDispatcher.onBackPressed() }
        compose.onNodeWithText("or2").assertIsDisplayed()
        compose.onNodeWithText("• Hosts").assertIsDisplayed()
        compose.onNodeWithText("Hosts fixture").assertIsDisplayed()
        compose.runOnIdle {
            assertSame(retained, holder.active.value)
            assertEquals(SessionState.Connected, retained!!.state.value)
            assertFalse(destroyed)
        }
        compose.onNodeWithText("Session").performClick()
        view = awaitTerminal()
        compose.onNodeWithText("or2").assertDoesNotExist()
        compose.onNodeWithText("Disconnect").performClick()
        compose.onNodeWithText("Close").assertIsDisplayed()
        compose.onNodeWithText("Disconnected").assertIsDisplayed()
        compose.onNodeWithText("or2").assertDoesNotExist()
        compose.onNodeWithText("Keys").assertDoesNotExist()
        compose.runOnIdle {
            assertSame(view, compose.activity.window.decorView.terminal())
            assertEquals("LR", view.grid.rows.single().cells.joinToString("") { it.text })
            assertFalse(destroyed)
        }
        compose.onNodeWithText("Close").performClick()
        compose.onNodeWithText("or2").assertIsDisplayed()
        compose.onNodeWithText("Hosts").assertIsDisplayed()
        compose.onNodeWithText("Keys").assertIsDisplayed()
        compose.onNodeWithText("• Session").assertIsDisplayed()
        compose.onNodeWithText("No active session. Choose Connect on a host to unlock its key.").assertIsDisplayed()
        compose.waitUntil(5_000) {
            var closed = false
            compose.runOnUiThread { closed = destroyed }
            closed
        }
        compose.runOnIdle { assertEquals(1, closes) }
    }
}
