package io.github.code_akram.or2.terminal

import android.content.ClipData
import android.content.ClipboardManager
import android.os.SystemClock
import android.view.MotionEvent
import android.view.View
import android.view.ViewGroup
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.toPixelMap
import androidx.compose.ui.test.captureToImage
import io.github.code_akram.or2.ffi.CursorShape
import io.github.code_akram.or2.ffi.TargetScroll
import io.github.code_akram.or2.ffi.TerminalModes
import io.github.code_akram.or2.ui.Or2Colors
import kotlin.math.abs
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.requiredWidth
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
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
import androidx.compose.ui.test.performScrollTo
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
import io.github.code_akram.or2.ffi.KeyModifiers
import io.github.code_akram.or2.ffi.SessionInterface
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalFrame
import io.github.code_akram.or2.ffi.TerminalKey
import io.github.code_akram.or2.ffi.ViewportScroll
import io.github.code_akram.or2.ui.Or2Theme
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.paste.ImageFormat
import io.github.code_akram.or2.paste.ImagePaste
import io.github.code_akram.or2.paste.PreparedImage
import io.github.code_akram.or2.paste.UploadState
import io.github.code_akram.or2.paste.uploadNotice
import kotlinx.coroutines.MainScope
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
        val clicks = mutableListOf<Pair<Int, Int>>()

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
        val pastes = mutableListOf<String>()
        override fun pasteText(text: String) {
            if (refuse) throw SessionException.NotConnected()
            pastes += text
        }
        override fun sendKey(input: KeyInput) {
            if (refuse) throw SessionException.NotConnected()
            keys += input
        }
        override fun resize(columns: UShort, rows: UShort) = Unit
        override fun scroll(scroll: ViewportScroll) { scrolls += scroll }
        override fun mouseClick(column: UShort, row: UShort) { clicks += column.toInt() to row.toInt() }
        override fun requestFullFrame() = Unit
        override fun takeFrame(): TerminalFrame? = null
        override fun state(): SessionState = SessionState.Connected
        override fun transport() = TerminalTransport.SSH
        override fun serverPid(): UInt? = null
        override fun clientId(): String? = null
        override fun roam() = Unit
        override fun approveHostKey(fingerprint: String) = Unit
        override fun rejectHostKey() = Unit
        override fun disconnect() = Unit
    }

    private val session = Recording()

    // The runner (Or2TestRunner) points the font-size preference at a scratch file for every device
    // test; each test here starts from the default size.
    @Before fun startFromTheDefaultSize() {
        instrumentation.targetContext.getSharedPreferences(Or2TestRunner.SCRATCH_FILE, 0).edit().clear().commit()
    }

    private fun show(
        pad: Boolean = false, composer: Boolean = false, state: SessionState = SessionState.Connected,
        chrome: TerminalChromeState = TerminalChromeState(padOpen = pad, composerOpen = composer), images: ImagePaste? = null,
    ) = compose.runOnUiThread {
        compose.activity.setContent {
            Or2Theme {
                TerminalScreen(session, MutableStateFlow(state), MutableSharedFlow(), Modifier.fillMaxSize(),
                    composerHint = "Message agent…", chrome = chrome, imagePaste = images)
            }
        }
    }

    /** An image paste whose uploads all land at [path] at once, without a host. */
    private fun images(path: String) = ImagePaste(MainScope()) { _, _ -> path }

    private val image = PreparedImage(byteArrayOf(1), ImageFormat.PNG)

    private fun armed() = SemanticsMatcher.expectValue(SemanticsProperties.StateDescription, "Armed for next key")
    private fun off() = SemanticsMatcher.expectValue(SemanticsProperties.StateDescription, "Off")

    @Test
    fun aTargetScrolledAwayBeforeTheViewExistedShowsTheButtonAlsoAfterTheSwapsNewView() {
        // The terminal's own scroller, scrolled up while another view (or none) showed it.
        val scroller = TargetScroller(MainScope(), { _ -> })
        var current by mutableStateOf<SessionInterface>(session)
        compose.runOnUiThread {
            scroller.scroll(-5)
            compose.activity.setContent {
                Or2Theme {
                    TerminalScreen(current, MutableStateFlow(SessionState.Connected), MutableSharedFlow(), Modifier.fillMaxSize(),
                        target = TerminalTarget.Tmux("main"), targetScroller = scroller, input = { current })
                }
            }
        }
        compose.onNodeWithTag("scroll-to-bottom").assertIsDisplayed()
        // The SSH-to-mosh swap: a new handle, so a new view, of the same terminal.
        compose.runOnUiThread { current = Recording() }
        compose.waitForIdle()
        compose.onNodeWithTag("scroll-to-bottom").assertIsDisplayed()
        compose.onNodeWithTag("scroll-to-bottom").performClick()
        compose.waitForIdle()
        compose.onNodeWithTag("scroll-to-bottom").assertDoesNotExist()
    }

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
        compose.onNodeWithTag("extra:-").performClick()
        compose.runOnIdle { assertEquals(listOf(TerminalKey.Home, TerminalKey.Character("-")), session.keys.map { it.key }) }
        compose.onNodeWithTag("extra:/").assertDoesNotExist() // `/` is on the toolbar itself.
    }

    @Test
    fun theArrowPadFloatsWithoutABackingAndTheToolbarKeyClosesIt() {
        show(pad = true)
        // No panel behind the keys and no grip: the keys float over the terminal on their own.
        compose.onNodeWithTag("pad-backing").assertDoesNotExist()
        compose.onNodeWithContentDescription("Collapse arrow pad").assertDoesNotExist()
        // The cluster is exactly its keys: 3 x 40 dp keys and two 6 dp gaps wide, nothing around them.
        val cluster = compose.onNodeWithTag("pad-cluster").fetchSemanticsNode().boundsInRoot
        val left = compose.onNodeWithTag("pad:Left").fetchSemanticsNode().boundsInRoot
        val right = compose.onNodeWithTag("pad:Right").fetchSemanticsNode().boundsInRoot
        assertEquals(cluster.left, left.left, 1f)
        assertEquals(cluster.right, right.right, 1f)
        assertEquals(cluster.top, compose.onNodeWithTag("pad:Up").fetchSemanticsNode().boundsInRoot.top, 1f)
        // The toolbar's arrow-pad key is the toggle that closes it again.
        compose.onNodeWithTag("key:Arrows").performClick()
        compose.onNodeWithTag("arrow-pad").assertDoesNotExist()
        compose.onNodeWithTag("key:Arrows").performClick()
        compose.onNodeWithTag("arrow-pad").assertIsDisplayed()
    }

    /** The colour at ([xDp], half height) of a node's own pixels. */
    private fun pixel(tag: String, xDp: Float): Color {
        val image = compose.onNodeWithTag(tag).captureToImage().toPixelMap()
        val density = instrumentation.targetContext.resources.displayMetrics.density
        return image[(xDp * density).toInt(), image.height / 2]
    }

    private fun Color.near(other: Color) =
        abs(red - other.red) < 0.03f && abs(green - other.green) < 0.03f && abs(blue - other.blue) < 0.03f

    @Test
    fun thePadKeysAreBlueAndNotTheTerminalBackground() {
        show(pad = true)
        compose.waitForIdle()
        // 6 dp in from a key's left edge: the fill, clear of the rounded corners and the 20 dp glyph.
        for (tag in listOf("pad:Up", "pad:Left", "pad:Down", "pad:Backspace")) {
            val fill = pixel(tag, 6f)
            assertFalse("$tag is the terminal's background", fill.near(Or2Colors.TerminalBackground))
            assertFalse("$tag is the old surface grey", fill.near(Or2Colors.Surface))
            assertTrue("$tag fill $fill is the pad's blue", fill.near(Or2Colors.PadKey))
            assertTrue("$tag fill $fill reads blue", fill.blue > fill.red + 0.1f)
        }
        // Enter is the primary key: filled accent.
        assertTrue(pixel("pad:Enter", 6f).near(Or2Colors.Accent))
    }

    private fun terminalView(): TerminalView {
        fun find(view: View): TerminalView? {
            if (view is TerminalView) return view
            if (view is ViewGroup) for (index in 0 until view.childCount) find(view.getChildAt(index))?.let { return it }
            return null
        }
        return find(compose.activity.window.decorView)!!
    }

    /** A herdr terminal showing a frame whose program tracks the mouse (herdr always does). */
    private fun showHerdrTrackingTheMouse(sent: MutableList<TargetScroll>): TargetScroller {
        lateinit var scroller: TargetScroller
        compose.runOnUiThread {
            scroller = TargetScroller(MainScope(), { scroll -> sent += scroll })
            compose.activity.setContent {
                Or2Theme {
                    TerminalScreen(session, MutableStateFlow(SessionState.Connected), MutableSharedFlow(), Modifier.fillMaxSize(),
                        target = TerminalTarget.Herdr(null, "w1:p1"), targetScroller = scroller)
                }
            }
        }
        compose.waitForIdle()
        compose.runOnUiThread {
            val view = terminalView()
            val frame = terminalVisualFrame(20u, 13u, CursorShape.BAR)
            assertTrue(view.grid.apply(frame.copy(modes = TerminalModes(mouseTracking = true, alternateScreen = true))))
        }
        return scroller
    }

    @Test
    fun aWheelSwipeOnAHerdrTargetShowsTheButtonAndTheButtonSendsBottom() {
        val sent = mutableListOf<TargetScroll>()
        val scroller = showHerdrTrackingTheMouse(sent)
        compose.onNodeWithTag("scroll-to-bottom").assertDoesNotExist()
        compose.runOnUiThread {
            val view = terminalView()
            val downTime = SystemClock.uptimeMillis()
            // The finger moves down: the content scrolls up, as wheel events (route 1).
            view.send(MotionEvent.obtain(downTime, downTime, MotionEvent.ACTION_DOWN, view.width / 2f, view.cellHeight * 2, 0))
            view.send(MotionEvent.obtain(downTime, downTime + 50, MotionEvent.ACTION_MOVE, view.width / 2f, view.cellHeight * 4, 0))
            view.send(MotionEvent.obtain(downTime, downTime + 100, MotionEvent.ACTION_MOVE, view.width / 2f, view.cellHeight * 7, 0))
            view.send(MotionEvent.obtain(downTime, downTime + 400, MotionEvent.ACTION_UP, view.width / 2f, view.cellHeight * 7, 0))
            assertTrue(session.scrolls.isNotEmpty() && session.scrolls.all { it is ViewportScroll.Wheel && it.rows < 0 })
            assertTrue("herdr may be in its history now", scroller.away)
            assertTrue("the wheel events went out; nothing else", sent.isEmpty())
        }
        compose.onNodeWithTag("scroll-to-bottom").assertIsDisplayed()
        compose.onNodeWithTag("scroll-to-bottom").performClick()
        compose.waitForIdle()
        compose.runOnIdle { assertEquals(listOf<TargetScroll>(TargetScroll.Bottom), sent) }
        compose.onNodeWithTag("scroll-to-bottom").assertDoesNotExist()
    }

    @Test
    fun aTapIsAClickWhileTheProgramTracksTheMouse() {
        showHerdrTrackingTheMouse(mutableListOf())
        compose.runOnUiThread {
            val view = terminalView()
            val downTime = SystemClock.uptimeMillis()
            val x = view.horizontalInset + view.cellWidth * 4.5f
            val y = view.cellHeight * 2.5f
            view.send(MotionEvent.obtain(downTime, downTime, MotionEvent.ACTION_DOWN, x, y, 0))
            view.send(MotionEvent.obtain(downTime, downTime + 40, MotionEvent.ACTION_UP, x, y, 0))
            assertEquals(listOf(4 to 2), session.clicks)
            assertTrue(session.keys.isEmpty() && session.texts.isEmpty())
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

    /** Whether the toolbar's key row could scroll: its scroll range (0 when every key fits). */
    private fun toolbarScrollRange(): Float =
        compose.onNodeWithTag("toolbar-keys").fetchSemanticsNode().config[SemanticsProperties.HorizontalScrollAxisRange].maxValue()

    @Test
    fun theToolbarFitsA411DpWidePhoneWithoutScrollingAlsoWhileSelecting() {
        compose.runOnUiThread {
            compose.activity.setContent {
                Or2Theme {
                    Box(Modifier.requiredWidth(411.dp).fillMaxHeight()) {
                        TerminalScreen(session, MutableStateFlow(SessionState.Connected), MutableSharedFlow(), Modifier.fillMaxSize())
                    }
                }
            }
        }
        compose.waitForIdle()
        // No panes key (the header's green disc opens that sheet) and no history key (a swipe pages back).
        compose.onNodeWithTag("key:Panes").assertDoesNotExist()
        compose.onNodeWithTag("key:History").assertDoesNotExist()
        listOf("Ctrl", "Esc", "Tab", "Arrows", "Paste", "ShiftTab", "Slash", "At", "Composer", "Keyboard").forEach {
            compose.onNodeWithTag("key:$it").assertIsDisplayed()
        }
        assertEquals(0f, toolbarScrollRange())
        // A selection: Copy and Clear lead, the typing keys give way, and the row still fits.
        compose.runOnUiThread {
            val view = terminalView()
            assertTrue(view.grid.apply(terminalVisualFrame(20u, 13u, CursorShape.BAR)))
            view.beginSelection(CellPosition(1, 1), word = true)
        }
        compose.onNodeWithTag("key:Copy").assertIsDisplayed()
        compose.onNodeWithTag("key:Clear").assertIsDisplayed()
        compose.onNodeWithTag("key:ShiftTab").assertDoesNotExist()
        assertEquals(0f, toolbarScrollRange())
    }

    @Test
    fun closingTheComposerGivesTheKeysBackToTheTerminal() {
        show()
        compose.onNodeWithTag("key:Composer").performClick()
        compose.onNodeWithTag("composer-input").assertIsDisplayed()
        compose.onNodeWithTag("composer-close").performClick()
        compose.runOnIdle { assertTrue("the terminal has the keys again", terminalView().hasFocus()) }
        // The toolbar toggle closes it the same way.
        compose.onNodeWithTag("key:Composer").performClick()
        compose.onNodeWithTag("key:Composer").performClick()
        compose.onNodeWithTag("composer-input").assertDoesNotExist()
        compose.runOnIdle { assertTrue(terminalView().hasFocus()) }
    }

    @Test
    fun thePadAndTheComposerAreNeverOpenTogether() {
        show()
        compose.onNodeWithTag("key:Composer").performClick()
        compose.onNodeWithTag("key:Arrows").performClick() // Opening the pad closes the composer ...
        compose.onNodeWithTag("arrow-pad").assertIsDisplayed()
        compose.onNodeWithTag("composer").assertDoesNotExist()
        compose.runOnIdle { assertTrue(terminalView().hasFocus()) }
        compose.onNodeWithTag("key:Composer").performClick() // ... and opening the composer closes the pad.
        compose.onNodeWithTag("composer").assertIsDisplayed()
        compose.onNodeWithTag("arrow-pad").assertDoesNotExist()
    }

    @Test
    fun aConfirmedMultiLinePasteGoesOutThroughPasteText() {
        compose.runOnUiThread {
            compose.activity.getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText("test", "first\nsecond"))
        }
        show()
        compose.onNodeWithTag("key:Paste").performClick()
        compose.onNodeWithText("Paste 2 lines?").assertIsDisplayed()
        compose.runOnIdle { assertTrue("nothing runs before the answer", session.pastes.isEmpty() && session.texts.isEmpty()) }
        compose.onNodeWithTag("paste-confirm").performClick()
        compose.runOnIdle {
            // The one paste path: the session's paste_text (bracketed when the program asks), never typed text.
            assertEquals(listOf("first\nsecond"), session.pastes)
            assertTrue(session.texts.isEmpty() && session.keys.isEmpty())
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
    fun theAttachButtonShowsOnlyForATerminalThatTakesImages() {
        show(composer = true)
        compose.onNodeWithTag("composer").assertIsDisplayed()
        compose.onNodeWithTag("composer-attach").assertDoesNotExist()
        show(composer = true, images = images("/p.png"))
        compose.onNodeWithTag("composer-attach").assertIsDisplayed().assertTouchTargetAtLeast(40)
        compose.onNodeWithContentDescription("Attach image").assertIsDisplayed()
        // Left of the text, on the same compact row.
        val (attach, input) = settledBounds("composer-attach", "composer-input")
        assertTrue(attach.right <= input.left + 1)
    }

    @Test
    fun anUploadedPathGoesIntoTheOpenComposerElseIntoTheTerminalAsAPasteWithoutEnter() {
        val chrome = TerminalChromeState(composerOpen = true, composerText = "look at")
        val paste = images("/home/u/my images/or2-1.png")
        show(chrome = chrome, images = paste)
        compose.runOnIdle { paste.start { image } }
        compose.waitUntil(5_000) { chrome.composerText == "look at '/home/u/my images/or2-1.png'" }
        compose.onNodeWithTag("composer-input").assertTextContains("look at '/home/u/my images/or2-1.png'")
        compose.runOnIdle { assertTrue("nothing typed into the terminal", session.pastes.isEmpty() && session.submits.isEmpty()) }
        // Closed composer: one paste into the terminal, a space first and no Enter after.
        compose.onNodeWithTag("composer-close").performClick()
        compose.runOnIdle { paste.start { image } }
        compose.waitUntil(5_000) { session.pastes.isNotEmpty() }
        compose.runOnIdle {
            assertEquals(listOf(" '/home/u/my images/or2-1.png'"), session.pastes)
            assertTrue(session.texts.isEmpty() && session.keys.isEmpty() && session.submits.isEmpty())
        }
    }

    @Test
    fun anUploadingImageCanBeCancelledFromTheNoticeStrip() {
        var cancelled = false
        compose.runOnUiThread {
            compose.activity.setContent {
                Or2Theme {
                    io.github.code_akram.or2.session.TerminalCard("workstation", "shell", Transport.SSH, SessionState.Connected,
                        minimise = {}, openSwitcher = {}, endSession = {}, upload = uploadNotice(UploadState.Uploading()),
                        uploadAction = { cancelled = true }) {}
                }
            }
        }
        compose.onNodeWithTag("upload-status").assertTextContains("Uploading image\u2026")
        compose.onNodeWithTag("upload-action").assertTextContains("Cancel").performClick()
        compose.runOnIdle { assertTrue(cancelled) }
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

    /** The program has bracketed paste on (a shell's line editor, an agent's TUI): its frames say so. */
    private fun bracketedPasteOn() {
        compose.waitForIdle()
        compose.runOnUiThread {
            val frame = terminalVisualFrame(20u, 13u, CursorShape.BAR)
            assertTrue(terminalView().grid.apply(frame.copy(modes = TerminalModes(false, false, bracketedPaste = true))))
        }
    }

    @Test
    fun severalLinesGoOutWithoutAskingWhileTheProgramHasBracketedPasteOn() {
        show(composer = true)
        bracketedPasteOn()
        compose.onNodeWithTag("composer-input").performTextInput("first\nsecond")
        compose.onNodeWithTag("composer-send").performClick()
        compose.onNodeWithText("Send 2 lines?").assertDoesNotExist()
        compose.runOnIdle {
            // One submit: Rust writes the lines as one paste, then one Enter.
            assertEquals(listOf("first\nsecond"), session.submits)
            assertTrue(session.texts.isEmpty() && session.keys.isEmpty())
        }
        compose.onNodeWithTag("composer-send").assertIsNotEnabled()
    }

    @Test
    fun shiftTabSlashAndAtFollowPasteAndGoWhereTheyShould() {
        show()
        // ⇧Tab is Shift+Tab whatever is latched, and leaves the latch for the next key.
        compose.onNodeWithTag("key:Ctrl").performClick().assert(armed())
        compose.onNodeWithTag("key:ShiftTab").performScrollTo().assertTextContains("⇧Tab").performClick()
        compose.runOnIdle { assertEquals(listOf(KeyInput(TerminalKey.Tab, KeyModifiers(true, false, false, false))), session.keys) }
        compose.onNodeWithTag("key:Ctrl").performScrollTo().assert(armed())
        // `/` with the composer closed is a key into the terminal: it takes the latch (Ctrl+/).
        compose.onNodeWithTag("key:Slash").performScrollTo().performClick()
        compose.runOnIdle { assertEquals(KeyInput(TerminalKey.Character("/"), KeyModifiers(false, true, false, false)), session.keys.last()) }
        compose.onNodeWithTag("key:Ctrl").performScrollTo().assert(off())
        compose.onNodeWithTag("key:At").performScrollTo().performClick()
        compose.runOnIdle { assertEquals(TerminalKey.Character("@"), session.keys.last().key) }
        // The composer open: `@` and `/` go into it at the cursor, nothing to the terminal.
        session.keys.clear()
        compose.onNodeWithTag("key:Composer").performClick()
        compose.onNodeWithTag("composer-input").performTextInput("ask ")
        compose.onNodeWithTag("key:At").performScrollTo().performClick()
        compose.onNodeWithTag("key:Slash").performScrollTo().performClick()
        compose.onNodeWithTag("composer-input").assertTextContains("ask @/")
        compose.runOnIdle { assertTrue(session.keys.isEmpty() && session.texts.isEmpty()) }
        // They follow Paste in the row, in this order.
        compose.waitForIdle()
        val left = listOf("Paste", "ShiftTab", "Slash", "At").map { compose.onNodeWithTag("key:$it").fetchSemanticsNode().positionInRoot.x }
        assertEquals(left.sorted(), left)
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
