package io.github.code_akram.or2.terminal

import android.content.ClipboardManager
import android.os.SystemClock
import android.text.InputType
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.View
import android.view.ViewGroup
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputMethodManager
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.github.code_akram.or2.ffi.KeyInput
import io.github.code_akram.or2.ffi.SessionException
import io.github.code_akram.or2.ffi.SessionInterface
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalFrame
import io.github.code_akram.or2.ffi.TerminalKey
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
        var error: SessionException? = null
        var snapshots = 0
        override fun sendText(text: String) { error?.let { throw it }; texts += text }
        override fun sendKey(input: KeyInput) { error?.let { throw it }; keys += input }
        override fun resize(columns: UShort, rows: UShort) { sizes += GridSize(columns, rows) }
        override fun scroll(scroll: ViewportScroll) { error?.let { throw it }; scrolls += scroll }
        override fun requestFullFrame() { snapshots++ }
        override fun takeFrame(): TerminalFrame? = null
        override fun state(): SessionState = SessionState.Connected
        override fun approveHostKey(fingerprint: String) = Unit
        override fun rejectHostKey() = Unit
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
            val connection = view.onCreateInputConnection(defaultInfo)
            assertEquals(InputType.TYPE_TEXT_VARIATION_NORMAL, defaultInfo.inputType and InputType.TYPE_MASK_VARIATION)
            view.directLatinInput = true
            val comparisonInfo = EditorInfo()
            view.onCreateInputConnection(comparisonInfo)
            assertEquals(InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD, comparisonInfo.inputType and InputType.TYPE_MASK_VARIATION)
            view.directLatinInput = false
            connection.setComposingText("not sent", 1)
            connection.finishComposingText()
            assertTrue(session.texts.isEmpty())
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
            assertTrue(session.texts.isEmpty())
            assertEquals(3, session.keys.size)
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
                    view.cellWidth * 2.5f, view.cellHeight * 1.5f, 0)
                view.dispatchTouchEvent(event)
                event.recycle()
            }
            SystemClock.sleep(650)
            scenario.onActivity { activity ->
                val view = activity.window.decorView.terminal()!!
                assertNotNull(view.selection)
                val move = MotionEvent.obtain(downTime, SystemClock.uptimeMillis(), MotionEvent.ACTION_MOVE,
                    view.cellWidth * 5.5f, view.cellHeight * 1.5f, 0)
                view.dispatchTouchEvent(move)
                move.recycle()
                val up = MotionEvent.obtain(downTime, SystemClock.uptimeMillis(), MotionEvent.ACTION_UP,
                    view.cellWidth * 5.5f, view.cellHeight * 1.5f, 0)
                view.dispatchTouchEvent(up)
                up.recycle()
                assertEquals("界😀e\u0301", view.selection!!.text())
                view.requestFocus()
                view.copySelection()
                val clipboard = activity.getSystemService(ClipboardManager::class.java)
                assertEquals("界😀e\u0301", clipboard.primaryClip!!.getItemAt(0).text.toString())
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
            await(scenario) { it.height < originalHeight && it.grid.rows.size == gridSize(it.width, it.height, it.cellWidth, it.cellHeight)!!.rows.toInt() }
            scenario.onActivity { activity ->
                val view = activity.window.decorView.terminal()!!
                activity.getSystemService(InputMethodManager::class.java).hideSoftInputFromWindow(view.windowToken, 0)
            }
            await(scenario) { it.height == originalHeight && it.grid.rows.size == gridSize(it.width, it.height, it.cellWidth, it.cellHeight)!!.rows.toInt() }
            scenario.recreate()
            await(scenario) { it.grid.hasGrid && it.rowText(0) == "or2 contract probe" }
        }
    }
}
