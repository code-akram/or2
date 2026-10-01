package io.github.code_akram.or2.terminal

import android.os.SystemClock
import android.view.MotionEvent
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.assert
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertTextContains
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import io.github.code_akram.or2.ffi.TerminalTransport
import org.junit.Assert.assertNull
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextInput
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.test.down
import androidx.compose.ui.test.up
import androidx.test.platform.app.InstrumentationRegistry
import io.github.code_akram.or2.MainActivity
import io.github.code_akram.or2.assertTouchTargetAtLeast
import io.github.code_akram.or2.Or2TestRunner
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.SessionException
import io.github.code_akram.or2.ffi.KeyInput
import io.github.code_akram.or2.ffi.SessionInterface
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalFrame
import io.github.code_akram.or2.ffi.TerminalKey
import io.github.code_akram.or2.ffi.ViewportScroll
import io.github.code_akram.or2.ui.Or2Theme
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test

/** The terminal's input chrome: key toolbar, arrow pad with auto-repeat, composer, and pinch-to-zoom. */
class TerminalChromeDeviceTest {
    @get:Rule val compose = createAndroidComposeRule<MainActivity>()
    private val instrumentation get() = InstrumentationRegistry.getInstrumentation()

    private class Recording : SessionInterface {
        val texts = mutableListOf<String>()
        val submits = mutableListOf<String>()
        val keys = mutableListOf<KeyInput>()
        val scrolls = mutableListOf<ViewportScroll>()

        /** When set, input is refused like a dropped session refuses it. */
        var refuse = false
        override fun sendText(text: String) {
            if (refuse) throw SessionException.NotConnected()
            texts += text
        }
        override fun submitText(text: String) {
            if (refuse) throw SessionException.NotConnected()
            submits += text
        }
        override fun sendKey(input: KeyInput) {
            if (refuse) throw SessionException.NotConnected()
            keys += input
        }
        override fun resize(columns: UShort, rows: UShort) = Unit
        override fun scroll(scroll: ViewportScroll) { scrolls += scroll }
        override fun requestFullFrame() = Unit
        override fun takeFrame(): TerminalFrame? = null
        override fun state(): SessionState = SessionState.Connected
        override fun transport() = TerminalTransport.SSH
        override fun serverPid(): UInt? = null
        override fun roam() = Unit
        override fun approveHostKey(fingerprint: String) = Unit
        override fun rejectHostKey() = Unit
        override fun disconnect() = Unit
    }

    private val session = Recording()
    private var panes = 0

    // The runner (Or2TestRunner) points the font-size preference at a scratch file for every device
    // test; each test here starts from the default size.
    @Before fun startFromTheDefaultSize() {
        instrumentation.targetContext.getSharedPreferences(Or2TestRunner.SCRATCH_FILE, 0).edit().clear().commit()
    }

    private fun show(pad: Boolean = false, composer: Boolean = false, state: SessionState = SessionState.Connected) = compose.runOnUiThread {
        compose.activity.setContent {
            Or2Theme {
                TerminalScreen(session, MutableStateFlow(state), MutableSharedFlow(), Modifier.fillMaxSize(),
                    composerHint = "Message agent…", openPanes = { panes++ }, chrome = TerminalChromeState(padOpen = pad, composerOpen = composer))
            }
        }
    }

    private fun armed() = SemanticsMatcher.expectValue(SemanticsProperties.StateDescription, "Armed for next key")
    private fun off() = SemanticsMatcher.expectValue(SemanticsProperties.StateDescription, "Off")

    @Test
    fun theToolbarSendsEscapeTabAndLatchesModifiersForExactlyOneKey() {
        show()
        compose.onNodeWithTag("key:Esc").performClick()
        compose.onNodeWithTag("key:Tab").performClick()
        compose.runOnIdle { assertEquals(listOf(TerminalKey.Escape, TerminalKey.Tab), session.keys.map { it.key }) }
        session.keys.clear()
        compose.onNodeWithTag("key:Ctrl").assert(off()).performClick().assert(armed())
        compose.onNodeWithTag("key:Arrows").performClick() // Open the arrow pad.
        compose.onNodeWithTag("pad:Up").performClick()
        compose.runOnIdle {
            assertEquals(TerminalKey.ArrowUp, session.keys.single().key)
            assertTrue(session.keys.single().modifiers.ctrl)
        }
        compose.onNodeWithTag("key:Ctrl").assert(off()) // The latch was spent on that key.
        compose.onNodeWithTag("key:Alt").performClick().assert(armed()).performClick().assert(off()) // Alt lives in the pad's extras row.
    }

    @Test
    fun theArrowPadHasTheThreeByThreeClusterAndTheExtras() {
        show(pad = true)
        compose.onNodeWithTag("arrow-pad").assertIsDisplayed()
        listOf("Backspace" to TerminalKey.Backspace, "Up" to TerminalKey.ArrowUp, "Left" to TerminalKey.ArrowLeft,
            "Enter" to TerminalKey.Enter, "Right" to TerminalKey.ArrowRight, "Down" to TerminalKey.ArrowDown).forEach { (tag, key) ->
            session.keys.clear()
            compose.onNodeWithTag("pad:$tag").performClick()
            compose.runOnIdle { assertEquals(key, session.keys.single().key) }
        }
        // Clear-line is Ctrl-U.
        session.keys.clear()
        compose.onNodeWithTag("pad:Clear").performClick()
        compose.runOnIdle {
            assertEquals(TerminalKey.Character("u"), session.keys.single().key)
            assertTrue(session.keys.single().modifiers.ctrl)
        }
        session.keys.clear()
        compose.onNodeWithTag("extra:Home").performClick()
        compose.onNodeWithTag("extra:/").performClick()
        compose.runOnIdle { assertEquals(listOf(TerminalKey.Home, TerminalKey.Character("/")), session.keys.map { it.key }) }
    }

    @Test
    fun theArrowPadKeysAndItsGripSitOnOneOpaqueBacking() {
        show(pad = true)
        val backing = compose.onNodeWithTag("pad-backing").fetchSemanticsNode().boundsInRoot
        val density = compose.activity.resources.displayMetrics.density
        val inside = listOf("pad:Backspace", "pad:Clear", "pad:Left", "pad:Right", "pad:Down").map {
            it to compose.onNodeWithTag(it).fetchSemanticsNode().boundsInRoot
        } + ("grip" to compose.onNodeWithContentDescription("Collapse arrow pad").fetchSemanticsNode().boundsInRoot)
        inside.forEach { (name, bounds) ->
            // Every key and the grip lie within the backing, with the 6 dp padding around them.
            assertTrue("$name $bounds in $backing", bounds.left >= backing.left + 5 * density && bounds.right <= backing.right - 5 * density)
            assertTrue("$name $bounds in $backing", bounds.top >= backing.top + 5 * density && bounds.bottom <= backing.bottom - 5 * density)
        }
    }

    @Test
    fun padKeysRepeatWhileHeldAndStopOnRelease() {
        show(pad = true)
        compose.waitForIdle()
        compose.mainClock.autoAdvance = false
        compose.onNodeWithTag("pad:Backspace").performTouchInput { down(center) }
        compose.mainClock.advanceTimeBy(100)
        compose.runOnIdle { assertEquals(1, session.keys.size) } // Pressed once at once ...
        compose.mainClock.advanceTimeBy(700)
        val held = compose.runOnIdle { session.keys.size }
        assertTrue("held key repeats ($held)", held >= 5)
        compose.onNodeWithTag("pad:Backspace").performTouchInput { up() }
        compose.mainClock.advanceTimeBy(500)
        compose.runOnIdle { assertEquals(held, session.keys.size) } // ... and not after the finger lifts.
        compose.mainClock.autoAdvance = true
        assertTrue(session.keys.all { it.key == TerminalKey.Backspace })
    }

    @Test
    fun theToolbarPanesAndHistoryKeysDoTheirJobs() {
        show()
        compose.onNodeWithTag("key:Panes").performClick()
        compose.onNodeWithTag("key:History").performClick()
        compose.runOnIdle {
            assertEquals(1, panes)
            assertTrue((session.scrolls.single() as ViewportScroll.Delta).rows < 0) // Up into the scrollback.
        }
    }

    @Test
    fun theComposerSubmitsTheTextAndTheSendButtonWaitsForText() {
        show()
        compose.onNodeWithTag("key:Composer").performClick()
        compose.onNodeWithTag("composer-input").assertIsDisplayed()
        compose.onNodeWithTag("composer-send").assertIsNotEnabled()
        compose.onNodeWithTag("composer-input").performTextInput("yes, go ahead")
        compose.onNodeWithTag("composer-send").assertIsEnabled().performClick()
        compose.runOnIdle {
            // One submit: Rust types the text and presses Enter itself, as a separate write.
            assertEquals(listOf("yes, go ahead"), session.submits)
            assertTrue(session.texts.isEmpty() && session.keys.isEmpty())
        }
        compose.onNodeWithTag("composer-send").assertIsNotEnabled() // Cleared after sending.
        compose.onNodeWithTag("key:Esc").assertIsDisplayed() // The toolbar stays: Esc, Ctrl and Tab are one tap away.
        compose.onNodeWithTag("key:Esc").performClick()
        compose.runOnIdle { assertEquals(TerminalKey.Escape, session.keys.last().key) }
        compose.onNodeWithTag("composer-close").performClick()
        compose.onNodeWithTag("composer-input").assertDoesNotExist()
        compose.onNodeWithTag("key:Esc").assertIsDisplayed()
    }

    @Test
    fun aMessageThatCouldNotBeSentStaysInTheComposer() {
        show(composer = true)
        session.refuse = true // The session dropped between typing and sending.
        compose.onNodeWithTag("composer-input").performTextInput("yes, go ahead")
        compose.onNodeWithTag("composer-send").assertIsEnabled().performClick()
        compose.onNodeWithTag("composer-input").assertTextContains("yes, go ahead")
        compose.onNodeWithTag("composer-send").assertIsEnabled()
        compose.runOnIdle { assertTrue(session.submits.isEmpty() && session.texts.isEmpty() && session.keys.isEmpty()) }
        session.refuse = false // Back up: the same text goes out on the next try, and only then is it cleared.
        compose.onNodeWithTag("composer-send").performClick()
        compose.runOnIdle {
            assertEquals(listOf("yes, go ahead"), session.submits)
            assertTrue(session.texts.isEmpty() && session.keys.isEmpty())
        }
        compose.onNodeWithTag("composer-send").assertIsNotEnabled()
    }

    @Test
    fun theComposerCannotSendWhileTheSessionIsNotConnected() {
        show(composer = true, state = SessionState.Closed(CloseReason.Disconnected))
        compose.onNodeWithTag("composer-input").performTextInput("too late")
        compose.onNodeWithTag("composer-send").assertIsNotEnabled()
        compose.runOnIdle { assertTrue(session.submits.isEmpty() && session.texts.isEmpty()) }
    }

    @Test
    fun severalLinesFromTheComposerAreConfirmedLikeAMultiLinePaste() {
        show(composer = true)
        compose.onNodeWithTag("composer-input").performTextInput("first\nsecond")
        compose.onNodeWithTag("composer-send").performClick()
        compose.onNodeWithText("Send 2 lines?").assertIsDisplayed()
        compose.runOnIdle { assertTrue("nothing runs before the answer", session.submits.isEmpty() && session.texts.isEmpty() && session.keys.isEmpty()) }
        compose.onNodeWithText("Cancel").performClick()
        compose.onNodeWithTag("composer-input").assertTextContains("first\nsecond") // Kept to edit or send again.
        compose.runOnIdle { assertTrue(session.submits.isEmpty()) }
        compose.onNodeWithTag("composer-send").performClick()
        compose.onNodeWithTag("composer-send-confirm").performClick()
        compose.runOnIdle {
            assertEquals(listOf("first\nsecond"), session.submits)
            assertTrue(session.texts.isEmpty() && session.keys.isEmpty())
        }
        compose.onNodeWithTag("composer-send").assertIsNotEnabled() // Cleared once it went out.
    }

    @Test
    fun theComposerIsOneCompactRowWithoutTheToolbarsDuplicateActions() {
        show(composer = true)
        compose.onNodeWithTag("composer-paste").assertDoesNotExist()
        compose.onNodeWithTag("composer-panes").assertDoesNotExist()
        val density = compose.activity.resources.displayMetrics.density
        // The IME and the composer's own layout are still settling right after `show`: measuring at once
        // caught a half-moved row on the phone (2 of 3 runs passed). Wait until the three bounds stop moving.
        val (composer, input, send) = settledBounds("composer", "composer-input", "composer-send")
        // One line of text and the actions share one row: about 40 dp (the old two-row card was 83 dp).
        assertTrue("composer is ${composer.height / density} dp tall", composer.height <= 52 * density)
        assertTrue("send sits right of the text on the same row", send.left >= input.right - 1 && send.bottom <= composer.bottom + 1 && send.top >= composer.top - 1)
    }

    /**
     * The bounds of the nodes tagged [tags] once none of them has moved for [QUIET_MS], waiting at most
     * [SETTLE_MS] (a bounded `waitUntil`, never a fixed sleep: it ends as soon as the layout is still).
     */
    private fun settledBounds(vararg tags: String): List<androidx.compose.ui.geometry.Rect> {
        var last: List<androidx.compose.ui.geometry.Rect>? = null
        var since = SystemClock.uptimeMillis()
        compose.waitUntil(timeoutMillis = SETTLE_MS) {
            val now = runCatching { tags.map { compose.onNodeWithTag(it).fetchSemanticsNode().boundsInRoot } }.getOrNull()
            if (now == null || now != last) {
                last = now
                since = SystemClock.uptimeMillis()
                false
            } else {
                SystemClock.uptimeMillis() - since >= QUIET_MS
            }
        }
        return checkNotNull(last)
    }

    @Test
    fun theComposerActionsHaveFullSizeTouchTargets() {
        show(composer = true)
        listOf("composer-close", "composer-send").forEach {
            compose.onNodeWithTag(it).assertTouchTargetAtLeast(48)
        }
    }

    private fun pointer2(downTime: Long, time: Long, action: Int, spread: Float): MotionEvent {
        val properties = Array(2) { MotionEvent.PointerProperties().apply { id = it; toolType = MotionEvent.TOOL_TYPE_FINGER } }
        val coords = Array(2) { MotionEvent.PointerCoords().apply { x = 720f + (if (it == 0) -spread else spread); y = 1000f; pressure = 1f; size = 1f } }
        return MotionEvent.obtain(downTime, time, action, 2, properties, coords, 0, 0, 1f, 1f, 0, 0, 0, 0)
    }

    private fun TerminalView.send(event: MotionEvent) {
        dispatchTouchEvent(event)
        event.recycle()
    }

    /** Two fingers down at [from] apart (first finger at x 560), spreading by [factor] per event. */
    private fun TerminalView.pinch(downTime: Long, from: Float, steps: Int, factor: Float): Long {
        val pointerDown = MotionEvent.ACTION_POINTER_DOWN or (1 shl MotionEvent.ACTION_POINTER_INDEX_SHIFT)
        send(MotionEvent.obtain(downTime, downTime, MotionEvent.ACTION_DOWN, 720f - from, 1000f, 0))
        var time = downTime + 10
        send(pointer2(downTime, time, pointerDown, from))
        var spread = from
        repeat(steps) {
            spread *= factor
            time += 16
            send(pointer2(downTime, time, MotionEvent.ACTION_MOVE, spread))
        }
        time += 16
        send(pointer2(downTime, time, MotionEvent.ACTION_POINTER_UP or (1 shl MotionEvent.ACTION_POINTER_INDEX_SHIFT), spread))
        return time
    }

    @Test
    fun pinchZoomChangesTheCellSizeAndIsRememberedForNewViews() {
        val context = instrumentation.targetContext
        lateinit var first: TerminalView
        instrumentation.runOnMainSync { first = TerminalView(context) }
        // The default is the small dense size: about 55 columns on a 411 dp wide phone.
        val density = context.resources.displayMetrics.density
        val columns = (411 * density / first.cellWidth).toInt()
        assertEquals(TerminalPrefs.DefaultFontSp, first.fontSizeSp, 0f)
        assertTrue("default size gives about 55 columns, not $columns", columns in 50..58)
        val before = first.cellWidth
        instrumentation.runOnMainSync {
            first.layout(0, 0, 1000, 1000)
            first.bind(session)
            // The detector ignores spans under about 27 mm, so the fingers start well apart on a 1440 px screen.
            first.pinch(SystemClock.uptimeMillis(), from = 160f, steps = 3, factor = 1.4f)
        }
        assertTrue("spreading two fingers zooms in (${first.fontSizeSp})", first.fontSizeSp > TerminalPrefs.DefaultFontSp)
        assertTrue(first.cellWidth > before)
        // A new view starts at the size the pinch ended at.
        lateinit var second: TerminalView
        instrumentation.runOnMainSync { second = TerminalView(context) }
        assertEquals(first.fontSizeSp, second.fontSizeSp, 0f)
        // Sizes are clamped.
        instrumentation.runOnMainSync {
            second.setFontSize(1000f)
            assertEquals(TerminalPrefs.MaxFontSp, second.fontSizeSp, 0f)
            second.setFontSize(0.5f)
            assertEquals(TerminalPrefs.MinFontSp, second.fontSizeSp, 0f)
        }
    }

    @Test
    fun aSlowPinchStillZoomsBecauseEachSmallStepAccumulates() {
        lateinit var view: TerminalView
        instrumentation.runOnMainSync { view = TerminalView(instrumentation.targetContext) }
        val sizes = mutableListOf<Float>()
        instrumentation.runOnMainSync {
            view.layout(0, 0, 1000, 1000)
            view.bind(session)
            // 70 events at 1.02x: one half step at 12 dp is about 4 %, so no single event is enough.
            view.pinch(SystemClock.uptimeMillis(), from = 160f, steps = 70, factor = 1.02f)
            sizes += view.fontSizeSp
        }
        assertTrue("a slow pinch must zoom in, not stall at ${sizes.single()}", sizes.single() >= TerminalPrefs.DefaultFontSp * 1.5f)
        // Neither a scroll, a selection nor input came out of the gesture.
        assertTrue(session.scrolls.isEmpty() && session.texts.isEmpty() && session.keys.isEmpty())
        assertNull(view.selection)
        // And slowly pinching back in shrinks it again.
        instrumentation.runOnMainSync {
            view.pinch(SystemClock.uptimeMillis(), from = 600f, steps = 70, factor = 0.98f)
        }
        assertTrue("pinching in zooms out (${view.fontSizeSp})", view.fontSizeSp < sizes.single())
        assertTrue(session.scrolls.isEmpty() && session.texts.isEmpty() && session.keys.isEmpty())
    }

    @Test
    fun theFingerLeftDownAfterAPinchDoesNotScrollTheTerminal() {
        lateinit var view: TerminalView
        instrumentation.runOnMainSync { view = TerminalView(instrumentation.targetContext) }
        instrumentation.runOnMainSync {
            view.layout(0, 0, 1000, 1000)
            view.bind(session)
            val downTime = SystemClock.uptimeMillis()
            var time = view.pinch(downTime, from = 160f, steps = 5, factor = 1.3f)
            // The second finger lifted; the first drags on and then lifts.
            repeat(4) { step ->
                time += 16
                view.send(MotionEvent.obtain(downTime, time, MotionEvent.ACTION_MOVE, 560f, 1000f + (step + 1) * view.cellHeight * 2, 0))
            }
            time += 16
            view.send(MotionEvent.obtain(downTime, time, MotionEvent.ACTION_UP, 560f, 1000f + 8 * view.cellHeight, 0))
            assertTrue("the leftover finger must not scroll: ${session.scrolls}", session.scrolls.isEmpty())
            // The next touch is an ordinary one again.
            val next = time + 100
            view.send(MotionEvent.obtain(next, next, MotionEvent.ACTION_DOWN, 560f, 500f, 0))
            view.send(MotionEvent.obtain(next, next + 50, MotionEvent.ACTION_MOVE, 560f, 500f + view.cellHeight * 4, 0))
            assertTrue("a normal drag scrolls again", session.scrolls.isNotEmpty())
        }
    }

    private companion object {
        /** How long the composer's bounds must stay put before they are measured. */
        const val QUIET_MS = 400L

        /** The most the layout may take to settle before the test fails. */
        const val SETTLE_MS = 8_000L
    }
}
