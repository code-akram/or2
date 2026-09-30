package io.github.code_akram.or2.session

import android.view.View
import android.view.ViewGroup
import androidx.activity.compose.setContent
import androidx.compose.material3.MaterialTheme
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import io.github.code_akram.or2.MainActivity
import io.github.code_akram.or2.data.HostRecord
import io.github.code_akram.or2.data.TrustStore
import io.github.code_akram.or2.ffi.*
import io.github.code_akram.or2.terminal.TerminalView
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertSame
import org.junit.Rule
import org.junit.Test

/** Render trust states without connecting to any host or modifying the production database. */
class EntryUiDeviceTest {
    @get:Rule val compose = createAndroidComposeRule<MainActivity>()

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
        compose.runOnUiThread {
            runBlocking { holder.connect(HostRecord(1, "Fixture", "fixture.invalid", 22, "fixture", null), byteArrayOf(1)) }
            compose.activity.setContent { MaterialTheme { SessionScreen(holder, false, { _, _ -> }, {}) } }
        }
        var view: TerminalView? = null
        compose.waitUntil(5_000) {
            var ready = false
            compose.runOnUiThread {
                view = compose.activity.window.decorView.terminal()
                ready = view?.grid?.hasGrid == true
            }
            ready
        }
        compose.onNodeWithText("Disconnect").performClick()
        compose.onNodeWithText("Close session").assertIsDisplayed()
        compose.runOnIdle {
            assertSame(view, compose.activity.window.decorView.terminal())
            assertEquals("LR", view!!.grid.rows.single().cells.joinToString("") { it.text })
            assertFalse(destroyed)
        }
        compose.onNodeWithText("Close session").performClick()
        compose.onNodeWithText("No active session. Choose Connect on a host to unlock its key.").assertIsDisplayed()
        compose.waitUntil(5_000) {
            var closed = false
            compose.runOnUiThread { closed = destroyed }
            closed
        }
        compose.runOnIdle { assertEquals(1, closes) }
    }
}
