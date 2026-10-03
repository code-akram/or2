package io.github.code_akram.or2.terminal

import android.content.ClipboardManager
import android.os.SystemClock
import android.text.InputType
import android.view.KeyCharacterMap
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.View
import android.view.ViewGroup
import android.view.WindowInsets
import android.view.inputmethod.BaseInputConnection
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputMethodManager
import android.widget.FrameLayout
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.github.code_akram.or2.ffi.CursorShape
import io.github.code_akram.or2.ffi.KeyInput
import io.github.code_akram.or2.ffi.SessionException
import io.github.code_akram.or2.ffi.SessionInterface
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalFrame
import io.github.code_akram.or2.ffi.TerminalKey
import io.github.code_akram.or2.ffi.TerminalTransport
import io.github.code_akram.or2.ffi.ViewportScroll
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class TerminalDeviceTest {
    private val instrumentation get() = InstrumentationRegistry.getInstrumentation()

    private fun View.terminal(): TerminalView? {
        if (this is TerminalView) return this
        if (this is ViewGroup) for (index in 0 until childCount) getChildAt(index).terminal()?.let { return it }
        return null
    }

    private fun await(scenario: ActivityScenario<TerminalProbeActivity>, predicate: (TerminalView) -> Boolean) {
        val deadline = SystemClock.uptimeMillis() + 8_000
        while (SystemClock.uptimeMillis() < deadline) {
            var ready = false
            scenario.onActivity { activity -> ready = activity.window.decorView.terminal()?.let(predicate) == true }
            if (ready) return
            SystemClock.sleep(20)
        }
        fail("Terminal condition did not become true")
    }

    private fun TerminalView.rowText(row: Int) = grid.rows[row].cells.joinToString("") { it.text }.trimEnd()

    @Test fun realProbeReceivesOnlyCommittedTextAndModifiedKeys() {
        ActivityScenario.launch(TerminalProbeActivity::class.java).use { scenario ->
            await(scenario) { it.grid.hasGrid }
            scenario.onActivity { activity ->
                val view = activity.window.decorView.terminal()!!
                assertTrue(view.isHardwareAccelerated)
                val info = EditorInfo()
                val connection = view.onCreateInputConnection(info)
                assertTrue(info.inputType and InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS != 0)
                assertEquals(0, info.inputType and InputType.TYPE_TEXT_FLAG_AUTO_CORRECT)
                connection.setComposingText("e\u0301界😀", 1)
                assertEquals("e\u0301界😀", view.input.composing)
                assertEquals("", view.rowText(2))
                connection.commitText("é\n", 1)
                assertEquals("", view.input.composing)
            }
            await(scenario) { it.rowText(2) == "text c3 a9 0d" }
            scenario.onActivity { activity ->
                val view = activity.window.decorView.terminal()!!
                view.input.toggleCtrl()
                view.onCreateInputConnection(EditorInfo()).commitText("c", 1)
            }
            await(scenario) { it.rowText(3) == "key Character(\"c\")+ctrl" }
        }
    }

    private class RecordingSession : SessionInterface {
        val texts = mutableListOf<String>()
        val keys = mutableListOf<KeyInput>()
        val sizes = mutableListOf<GridSize>()
        val scrolls = mutableListOf<ViewportScroll>()
        val clicks = mutableListOf<Pair<Int, Int>>()
        var error: SessionException? = null
        var snapshots = 0
        var fullSnapshot: TerminalFrame? = null
        var pending: TerminalFrame? = null
        var deferSnapshot = false
        var onFrameReady: () -> Unit = {}
        private var sequence = 0uL
        override fun sendText(text: String) { error?.let { throw it }; texts += text }
        override fun submitText(text: String) { error?.let { throw it } }
        override fun pasteText(text: String) { error?.let { throw it } }
        override fun sendKey(input: KeyInput) { error?.let { throw it }; keys += input }
        // Deliberately no resize output: remount must recover via requestFullFrame, not resize.
        override fun resize(columns: UShort, rows: UShort) { sizes += GridSize(columns, rows) }
        override fun scroll(scroll: ViewportScroll) { error?.let { throw it }; scrolls += scroll }
        override fun mouseClick(column: UShort, row: UShort) { error?.let { throw it }; clicks += column.toInt() to row.toInt() }
        override fun requestFullFrame() {
            snapshots++
            if (!deferSnapshot) fullSnapshot?.let { publish(it.copy(sequence = ++sequence)) }
        }
        fun publish(frame: TerminalFrame) { pending = frame; onFrameReady() }
        override fun takeFrame(): TerminalFrame? = pending.also { pending = null }
        override fun state(): SessionState = SessionState.Connected
        override fun transport() = TerminalTransport.SSH
        override fun serverPid(): UInt? = null
        override fun clientId(): String? = null
        override fun roam() = Unit
        override fun disconnect() = Unit
    }

    @Test fun inputConnectionHardwareModifiersReleaseDeleteAndClosedAreSafe() {
        instrumentation.runOnMainSync {
            val view = TerminalView(instrumentation.targetContext)
            val session = RecordingSession()
            view.bind(session)
            view.sessionState(SessionState.Connected)
            assertEquals(1, session.snapshots)
            val defaultInfo = EditorInfo()
            view.onCreateInputConnection(defaultInfo)
            assertEquals(InputType.TYPE_TEXT_VARIATION_NORMAL, defaultInfo.inputType and InputType.TYPE_MASK_VARIATION)
            view.directLatinInput = true
            val comparisonInfo = EditorInfo()
            view.onCreateInputConnection(comparisonInfo)
            assertEquals(InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD, comparisonInfo.inputType and InputType.TYPE_MASK_VARIATION)
            view.directLatinInput = false
            val connection = view.onCreateInputConnection(EditorInfo())
            connection.setComposingText("finished", 1)
            assertTrue(session.texts.isEmpty())
            connection.finishComposingText()
            connection.finishComposingText()
            assertEquals(listOf("finished"), session.texts)
            connection.deleteSurroundingText(2, 0)
            assertEquals(listOf(TerminalKey.Backspace, TerminalKey.Backspace), session.keys.map { it.key })
            val down = KeyEvent(0, 0, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_C, 0, KeyEvent.META_CTRL_ON or KeyEvent.META_ALT_ON)
            assertTrue(connection.sendKeyEvent(down))
            assertEquals(TerminalKey.Character("c"), session.keys.last().key)
            assertTrue(session.keys.last().modifiers.ctrl)
            assertTrue(session.keys.last().modifiers.alt)
            connection.sendKeyEvent(KeyEvent.changeAction(down, KeyEvent.ACTION_UP))
            assertEquals(3, session.keys.size)
            session.error = SessionException.NotConnected()
            connection.commitText("early", 1)
            session.error = SessionException.Closed()
            connection.commitText("late", 1)
            connection.deleteSurroundingText(1, 0)
            view.jumpToBottom()
            assertEquals(listOf("finished"), session.texts)
            assertEquals(3, session.keys.size)
            session.error = null // A live session would accept text, but editor teardown must not send.
            connection.setComposingText("discard on close", 1)
            connection.closeConnection()
            connection.finishComposingText()
            assertEquals(listOf("finished"), session.texts)
        }
    }

    @Test fun pasteRetiresComposingEditorAndFreshCompositionCommitsAfterLiteralText() {
        instrumentation.runOnMainSync {
            val view = TerminalView(instrumentation.targetContext)
            val session = RecordingSession()
            view.bind(session)
            val old = view.onCreateInputConnection(EditorInfo()) as TerminalInputConnection
            old.setComposingText("にほん", 1)
            val editable = old.getEditable()
            BaseInputConnection.setComposingSpans(editable)
            assertEquals(0, BaseInputConnection.getComposingSpanStart(editable))
            assertEquals(3, BaseInputConnection.getComposingSpanEnd(editable))
            view.input.toggleCtrl()
            view.input.toggleAlt()
            view.paste("")
            assertEquals("にほん", editable.toString())
            assertEquals("にほん", view.input.composing)
            assertTrue(session.texts.isEmpty())
            view.paste("echo pasted\n")
            assertEquals("", editable.toString())
            assertEquals(-1, BaseInputConnection.getComposingSpanStart(editable))
            assertEquals(-1, BaseInputConnection.getComposingSpanEnd(editable))
            assertEquals(0, editable.getSpans(0, editable.length, Any::class.java).size)
            assertEquals("", view.input.composing)
            assertEquals(listOf("echo pasted\n"), session.texts)
            assertTrue(view.input.ctrl)
            assertTrue(view.input.alt)
            assertFalse(old.setComposingText("にほんご", 1))
            assertFalse(old.setComposingRegion(0, 0))
            assertFalse(old.commitText("日本語", 1))
            assertFalse(old.finishComposingText())
            assertFalse(old.deleteSurroundingText(1, 0))
            assertFalse(old.sendKeyEvent(KeyEvent(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_ENTER)))
            assertEquals(listOf("echo pasted\n"), session.texts)
            assertTrue(session.keys.isEmpty())
            view.input.toggleCtrl()
            view.input.toggleAlt()
            val fresh = view.onCreateInputConnection(EditorInfo())
            assertTrue(fresh.setComposingText("あたらしい", 1))
            old.closeConnection() // An asynchronous close must not erase the fresh overlay.
            assertEquals("あたらしい", view.input.composing)
            assertTrue(fresh.commitText("新しい", 1))
            fresh.finishComposingText()
            assertEquals(listOf("echo pasted\n", "新しい"), session.texts)
            assertTrue(session.keys.isEmpty())
            assertEquals("", view.input.composing)
        }
    }

    @Test fun remountSameDrainedHandleRequestsFullFrameAndRejectsAnEarlyDelta() {
        ActivityScenario.launch(TerminalProbeActivity::class.java).use { scenario ->
            val snapshot = terminalVisualFrame(8u, 14u, CursorShape.BAR)
            val session = RecordingSession().apply { fullSnapshot = snapshot }
            lateinit var container: FrameLayout
            lateinit var current: TerminalView
            fun mount(activity: TerminalProbeActivity) {
                current = TerminalView(activity).apply { bind(session) }
                session.onFrameReady = current::frameReady
                container.addView(current, FrameLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))
                current.sessionState(SessionState.Connected)
            }
            scenario.onActivity { activity ->
                container = FrameLayout(activity)
                activity.setContentView(container)
                mount(activity)
            }
            await(scenario) { it.grid.hasGrid && it.grid.sequence == 1uL }
            scenario.onActivity { activity ->
                assertNull(session.pending) // Initial frame consumed; no further driver output.
                assertEquals(1, session.snapshots)
                container.removeAllViews()
                mount(activity) // Reuses precisely the same connected handle, not another probe.
                assertEquals(2, session.snapshots)
            }
            await(scenario) { it.grid.hasGrid && it.grid.sequence == 2uL }
            scenario.onActivity { activity ->
                assertNull(session.pending)
                container.removeAllViews()
                session.deferSnapshot = true
                session.publish(snapshot.copy(sequence = 3u, full = false, changedRows = snapshot.changedRows.take(1)))
                mount(activity)
                assertEquals(3, session.snapshots)
            }
            await(scenario) { !it.grid.hasGrid && session.pending == null }
            scenario.onActivity {
                assertFalse(current.grid.hasGrid) // A delta must not manufacture a partial grid.
                assertEquals(3, session.snapshots) // Already awaiting the requested full snapshot.
                session.publish(snapshot.copy(sequence = 4u))
            }
            await(scenario) { it.grid.hasGrid && it.grid.sequence == 4uL }
            scenario.onActivity {
                assertEquals(8, current.grid.columns)
                assertEquals(14, current.grid.rows.size)
                assertEquals("e\u0301", current.grid.rows[11].cells[5].text)
                assertEquals(3, session.snapshots)
            }
        }
    }

    @Test fun hardwareKeysBypassImeExactlyOnceWithKeyboardShownAndHidden() {
        ActivityScenario.launch(TerminalProbeActivity::class.java).use { scenario ->
            val session = RecordingSession()
            scenario.onActivity { activity ->
                // The probe's view already owns its native session. Replace the test host,
                // not that view's binding, and wait for the new editor to be laid out/focused.
                activity.setContentView(TerminalView(activity).apply {
                    bind(session)
                    sessionState(SessionState.Connected)
                })
            }
            await(scenario) { it.isAttachedToWindow && it.width > 0 && it.hasWindowFocus() }
            scenario.onActivity { activity ->
                val view = activity.terminalView()!!
                view.requestFocus()
                val now = SystemClock.uptimeMillis()
                val raw = KeyEvent(now, now, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_B, 0)
                assertTrue(view.dispatchKeyEventPreIme(raw))
                assertTrue(view.dispatchKeyEventPreIme(KeyEvent.changeAction(raw, KeyEvent.ACTION_UP)))
                assertEquals(listOf(TerminalKey.Character("b")), session.keys.map { it.key })
                val soft = KeyEvent(now, now, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_Z, 0, 0,
                    KeyCharacterMap.VIRTUAL_KEYBOARD, 0, KeyEvent.FLAG_SOFT_KEYBOARD)
                assertFalse(view.dispatchKeyEventPreIme(soft))
                view.onCreateInputConnection(EditorInfo()).sendKeyEvent(soft)
                assertEquals(listOf(TerminalKey.Character("b"), TerminalKey.Character("z")), session.keys.map { it.key })
                session.keys.clear()
                view.showKeyboard()
            }
            await(scenario) { it.rootWindowInsets?.isVisible(WindowInsets.Type.ime()) == true }
            fun injectAndAssertOnce() {
                KeyCharacterMap.load(KeyCharacterMap.VIRTUAL_KEYBOARD).getEvents("clear".toCharArray())
                    .forEach { instrumentation.sendKeySync(it) }
                instrumentation.waitForIdleSync()
                SystemClock.sleep(200) // Also catch a delayed IME composition/commit echo.
                scenario.onActivity {
                    assertEquals("Hardware text must not be committed again by the IME", emptyList<String>(), session.texts)
                    assertEquals("clear".map { TerminalKey.Character(it.toString()) }, session.keys.map { it.key })
                    session.keys.clear()
                }
            }
            injectAndAssertOnce()
            scenario.onActivity { activity ->
                val view = activity.terminalView()!!
                activity.getSystemService(InputMethodManager::class.java).hideSoftInputFromWindow(view.windowToken, 0)
            }
            await(scenario) { it.rootWindowInsets?.isVisible(WindowInsets.Type.ime()) == false }
            injectAndAssertOnce()
            scenario.onActivity { activity ->
                assertFalse("Hardware typing must not reopen the soft keyboard",
                    activity.terminalView()!!.rootWindowInsets.isVisible(WindowInsets.Type.ime()))
            }
        }
    }

    @Test fun longPressDragCopiesWideAndCombiningCellsAndDragScrolls() {
        ActivityScenario.launch(TerminalProbeActivity::class.java).use { scenario ->
            await(scenario) { it.grid.hasGrid }
            var downTime = 0L
            scenario.onActivity { activity ->
                val view = activity.window.decorView.terminal()!!
                downTime = SystemClock.uptimeMillis()
                val event = MotionEvent.obtain(downTime, downTime, MotionEvent.ACTION_DOWN,
                    view.cellWidth * 2.5f + view.horizontalInset, view.cellHeight * 1.5f, 0)
                view.dispatchTouchEvent(event)
                event.recycle()
            }
            SystemClock.sleep(650)
            scenario.onActivity { activity ->
                val view = activity.window.decorView.terminal()!!
                assertNotNull(view.selection)
                assertEquals("R界😀e\u0301I", view.selection!!.text()) // Entire word, before any drag.
                val move = MotionEvent.obtain(downTime, SystemClock.uptimeMillis(), MotionEvent.ACTION_MOVE,
                    view.cellWidth * 5.5f + view.horizontalInset, view.cellHeight * 1.5f, 0)
                view.dispatchTouchEvent(move)
                move.recycle()
                val up = MotionEvent.obtain(downTime, SystemClock.uptimeMillis(), MotionEvent.ACTION_UP,
                    view.cellWidth * 5.5f + view.horizontalInset, view.cellHeight * 1.5f, 0)
                view.dispatchTouchEvent(up)
                up.recycle()
                assertEquals("R界😀e\u0301I", view.selection!!.text())
                view.requestFocus()
                view.copySelection()
                val clipboard = activity.getSystemService(ClipboardManager::class.java)
                assertEquals("R界😀e\u0301I", clipboard.primaryClip!!.getItemAt(0).text.toString())
                assertNull(view.selection)
                val session = RecordingSession()
                val gestureView = TerminalView(activity).apply { bind(session) }
                fun touch(action: Int, time: Long, y: Float) {
                    val event = MotionEvent.obtain(downTime, time, action, 20f, y, 0)
                    gestureView.dispatchTouchEvent(event)
                    event.recycle()
                }
                touch(MotionEvent.ACTION_DOWN, downTime, 100f)
                touch(MotionEvent.ACTION_MOVE, downTime + 100, 100f + gestureView.cellHeight * 4)
                assertTrue((session.scrolls.single() as ViewportScroll.Delta).rows < 0)
                gestureView.jumpToBottom()
                assertEquals(ViewportScroll.Bottom, session.scrolls.last())
            }
        }
    }

    @Test fun imeLayoutResizesNativeGridAndRecreatedViewRequestsFullSnapshot() {
        ActivityScenario.launch(TerminalProbeActivity::class.java).use { scenario ->
            await(scenario) { it.grid.hasGrid }
            var originalHeight = 0
            scenario.onActivity { activity ->
                val view = activity.window.decorView.terminal()!!
                originalHeight = view.height
                view.showKeyboard()
            }
            await(scenario) { it.height < originalHeight && it.grid.rows.size == it.currentGridSize()!!.rows.toInt() }
            scenario.onActivity { activity ->
                val view = activity.window.decorView.terminal()!!
                activity.getSystemService(InputMethodManager::class.java).hideSoftInputFromWindow(view.windowToken, 0)
            }
            await(scenario) { it.height == originalHeight && it.grid.rows.size == it.currentGridSize()!!.rows.toInt() }
            scenario.recreate()
            await(scenario) { it.grid.hasGrid && it.rowText(0) == "or2 contract probe" }
        }
    }
}
