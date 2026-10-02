package io.github.code_akram.or2.terminal

import android.content.Context
import android.content.ClipData
import android.content.ClipboardManager
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Path
import android.graphics.Picture
import android.graphics.RectF
import android.graphics.Typeface
import android.text.InputType
import android.util.LruCache
import android.util.TypedValue
import android.view.Choreographer
import android.view.GestureDetector
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.ScaleGestureDetector
import android.view.View
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputMethodManager
import android.widget.OverScroller
import androidx.compose.ui.graphics.toArgb
import io.github.code_akram.or2.ffi.CellStyle
import io.github.code_akram.or2.ffi.CellWidth
import io.github.code_akram.or2.ffi.CursorShape
import io.github.code_akram.or2.ffi.KeyModifiers
import io.github.code_akram.or2.ffi.SessionInterface
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.Underline
import io.github.code_akram.or2.ffi.KeyInput
import io.github.code_akram.or2.ffi.TargetScroll
import io.github.code_akram.or2.ffi.TerminalKey
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.ffi.ViewportScroll
import io.github.code_akram.or2.ui.Or2Colors
import io.github.code_akram.or2.ui.Or2Dimens
import kotlinx.coroutines.CoroutineScope
import kotlin.math.ceil
import kotlin.math.roundToInt

/** Canvas is the only renderer. The cache retains glyph commands, not terminal bitmaps. */
class TerminalView(context: Context) : View(context) {
    val grid = TerminalGrid()
    val applyTimings = FrameTimings()
    /** CPU display-list recording only; Window frame metrics measure the render pipeline. */
    val drawTimings = FrameTimings()
    /** Main-thread, unclipped Compose layout bounds of the key toolbar for content-free device diagnostics. */
    internal var toolbarBounds: RectF? = null
    internal val toolbarKeyBounds = mutableMapOf<String, RectF>()
    var showTimings = false
    // Debug comparison only. Production keeps composition-capable text mode; immediate
    // single-letter delivery with the phone's default IME passed real-SSH acceptance.
    internal var directLatinInput = false
    private val paint = Paint(Paint.ANTI_ALIAS_FLAG)
    /**
     * The font size in density-independent pixels (not scaled by the system font size, so the
     * column count is predictable); pinch changes it and it is remembered per device ([TerminalPrefs]).
     */
    var fontSizeSp = TerminalPrefs.load(context)
        private set
    private val textPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        typeface = Typeface.MONOSPACE
        textSize = TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, fontSizeSp, resources.displayMetrics)
    }
    private val baseTypeface = terminalTypeface(textPaint)
    internal val fontHasMonospacedAdvances = textPaint.hasMonospacedAdvances()
    private val typefaces = Array(4) { style ->
        // The single file has no bold face. Request only italic from it and synthesize bold
        // ourselves; Typeface.create(..., BOLD).isBold can describe a request, not a real face.
        val resolvedStyle = if (baseTypeface != Typeface.MONOSPACE) style and Typeface.BOLD.inv() else style
        if (resolvedStyle == Typeface.NORMAL) baseTypeface else Typeface.create(baseTypeface, resolvedStyle)
    }
    internal val boldUsesFake = !typefaces[Typeface.BOLD].isBold
    var cellWidth = ceil(textPaint.measureText("M"))
        private set
    var cellHeight = ceil(textPaint.fontMetrics.bottom - textPaint.fontMetrics.top)
        private set
    private var baseline = -textPaint.fontMetrics.top
    private data class Glyph(
        val text: String, val wide: Boolean, val foreground: UInt,
        val bold: Boolean, val italic: Boolean, val faint: Boolean, val fakeBold: Boolean,
    )
    private val glyphs = LruCache<Glyph, Picture>(2048)
    var onInputChanged: () -> Unit = {}
    var onSelectionChanged: () -> Unit = {}

    /** Called with the terminal's default background (0xRRGGBB) when a frame changes it (OSC 11). */
    var onBackgroundChanged: (UInt) -> Unit = {}
    private var reportedBackground = grid.background

    /**
     * Space kept free on the left and the right, so glyphs never touch the screen edge or sit under
     * a curved bezel. The grid's column count is measured inside it; the background fills it.
     */
    val horizontalInset = Or2Dimens.TerminalInset.value * resources.displayMetrics.density
    private var inputConnection: TerminalInputConnection? = null
    val input = TerminalInput(
        { text -> clearSelection(); atBottom { sessionCall { sendText(text) } } },
        { key -> clearSelection(); atBottom { sessionCall { sendKey(key) } } },
        { invalidate(); onInputChanged() },
    )
    var selection: TerminalSelection? = null
        private set
    private val scroller = OverScroller(context)
    private var flingY = 0
    private var scrollRemainder = 0f

    /** The cell a swipe (and the fling after it) started on: where its wheel events are reported. */
    private var scrollCell = CellPosition(0, 0)

    /** What this terminal shows: tmux and herdr targets scroll their own history ([scrollRoute]). */
    var target: TerminalTarget = TerminalTarget.Shell
        private set

    /** Scrolls a tmux or herdr target through `scroll_target`; null until the screen provides one. */
    var targetScroller: TargetScroller? = null
        private set

    /** Called with whether the scroll-to-bottom button should show, when that changes. */
    var onScrolledAwayChanged: (Boolean) -> Unit = {}
    private var reportedAway = false
    private val gestures = GestureDetector(context, object : GestureDetector.SimpleOnGestureListener() {
        override fun onDown(e: MotionEvent): Boolean {
            scroller.forceFinished(true)
            scrollCell = position(e.x, e.y) ?: scrollCell
            return true
        }
        override fun onSingleTapUp(e: MotionEvent): Boolean {
            clearSelection()
            performClick()
            showKeyboard()
            return true
        }
        override fun onLongPress(e: MotionEvent) {
            beginSelection(position(e.x, e.y) ?: return, word = true)
        }
        override fun onScroll(e1: MotionEvent?, e2: MotionEvent, distanceX: Float, distanceY: Float): Boolean {
            if (selection == null) scrollPixels(distanceY)
            return true
        }
        override fun onFling(e1: MotionEvent?, e2: MotionEvent, velocityX: Float, velocityY: Float): Boolean {
            if (selection != null) return true
            flingY = 0
            scroller.fling(0, 0, 0, -velocityY.toInt(), 0, 0, -1_000_000, 1_000_000)
            postInvalidateOnAnimation()
            return true
        }
    })
    private var pinching = false

    /** True from the start of a pinch until all fingers are up: the finger left over must not scroll. */
    private var pinchedSinceDown = false

    /** The continuous size a pinch is at; the applied size is this rounded to half steps. */
    private var pinchSize = TerminalPrefs.DefaultFontSp
    private val resizeAfterPinch = Runnable { resizeSession() }
    private val scaleGestures = ScaleGestureDetector(context, object : ScaleGestureDetector.SimpleOnScaleGestureListener() {
        override fun onScaleBegin(detector: ScaleGestureDetector): Boolean {
            pinching = true
            pinchedSinceDown = true
            pinchSize = fontSizeSp
            scroller.forceFinished(true)
            // The two fingers must never also be a scroll or a long-press selection.
            val cancel = MotionEvent.obtain(0, 0, MotionEvent.ACTION_CANCEL, 0f, 0f, 0)
            gestures.onTouchEvent(cancel)
            cancel.recycle()
            return true
        }

        override fun onScale(detector: ScaleGestureDetector): Boolean {
            // scaleFactor is relative to the previous event, which is about 1.01 to 1.03 for a slow
            // pinch: at a small font one half step is about 4 %, so rounding each event on its own
            // would discard it. The continuous size accumulates; only the applied size is stepped.
            pinchSize = (pinchSize * detector.scaleFactor).coerceIn(TerminalPrefs.MinFontSp, TerminalPrefs.MaxFontSp)
            // Half-dp steps keep cell metrics from jittering while the fingers move.
            val stepped = (pinchSize * 2).roundToInt() / 2f
            if (stepped != fontSizeSp) {
                setFontSize(stepped, resize = false)
                removeCallbacks(resizeAfterPinch)
                postDelayed(resizeAfterPinch, 120)
            }
            return true
        }

        override fun onScaleEnd(detector: ScaleGestureDetector) {
            pinching = false
            removeCallbacks(resizeAfterPinch)
            resizeSession()
            TerminalPrefs.save(context, fontSizeSp)
        }
    }).apply { isQuickScaleEnabled = false }
    private val session = TerminalSession()
    private var connected = false
    private var lastSize: GridSize? = null
    private var framePending = false
    private var requestedFull = false

    /**
     * Called after a frame this view applied has been drawn and committed, for every such frame (the
     * timing markers decide whether anyone is waiting: a retained view is shown again later).
     */
    var onFrameDrawn: () -> Unit = {}
    private val appliedFrames = AppliedFrames()
    private var cursorVisible = true
    private val blink = object : Runnable {
        override fun run() {
            cursorVisible = !cursorVisible
            if (grid.cursor?.blinking == true) invalidate()
            postDelayed(this, 500)
        }
    }
    private val frameCallback = Choreographer.FrameCallback {
        framePending = false
        val start = System.nanoTime()
        session.takeFrame()?.let { frame ->
            if (grid.apply(frame)) {
                requestedFull = false
                cursorVisible = true
                if (grid.background != reportedBackground) {
                    reportedBackground = grid.background
                    onBackgroundChanged(grid.background)
                }
                appliedFrames.applied()
                updateScrolledAway()
                invalidate()
            } else {
                requestSnapshot()
            }
            applyTimings.record(System.nanoTime() - start)
        }
    }

    init {
        isFocusable = true
        isFocusableInTouchMode = true
        contentDescription = "Terminal"
    }

    fun bind(session: SessionInterface) {
        this.session.bind(session)
        resizeSession()
    }

    fun sessionState(state: SessionState) {
        connected = !session.gone && state == SessionState.Connected
        if (connected && !grid.hasGrid) requestSnapshot()
    }

    private fun requestSnapshot() {
        if (!connected || session.gone || requestedFull) return
        if (sessionCall { requestFullFrame() }) requestedFull = true
    }

    fun frameReady() {
        if (!session.gone && !framePending && isAttachedToWindow) {
            framePending = true
            Choreographer.getInstance().postFrameCallback(frameCallback)
        }
    }

    internal fun sessionCall(block: SessionInterface.() -> Unit): Boolean = session.call(block)

    fun hideKeyboard() {
        context.getSystemService(InputMethodManager::class.java).hideSoftInputFromWindow(windowToken, 0)
    }

    /**
     * The composer's send: Rust types [text] and presses Enter as a separate write after a short
     * pause (`submit_text`), which agent TUIs with paste-burst detection need to see a submit
     * rather than a pasted newline. Pending Ctrl/Alt latches do not apply to a composed message.
     * Returns whether the session took it: false when it is closed or gone, so the caller keeps
     * the message.
     */
    fun sendLine(text: String): Boolean {
        if (text.isEmpty()) return false
        clearSelection()
        input.discardComposition()
        var taken: Boolean? = null
        atBottom { taken = sessionCall { submitText(text) } }
        // Held behind the target's Bottom: it goes out once tmux or herdr is back at the live screen.
        return taken ?: (connected && !session.gone)
    }

    fun showKeyboard() {
        requestFocus()
        context.getSystemService(InputMethodManager::class.java).showSoftInput(this, InputMethodManager.SHOW_IMPLICIT)
    }

    /** Cancel the editor as well as the overlay before sending literal clipboard text. */
    fun paste(text: String) {
        if (text.isEmpty()) return
        inputConnection?.cancelComposition()
        inputConnection = null
        input.discardComposition()
        context.getSystemService(InputMethodManager::class.java).restartInput(this)
        input.paste(text)
    }

    override fun onCheckIsTextEditor() = true

    override fun onCreateInputConnection(outAttrs: EditorInfo): InputConnection {
        // Text + multiline retains CJK/dead-key composition. NO_SUGGESTIONS and omission of
        // AUTO_CORRECT prevent command rewriting; password types would break some IMEs.
        outAttrs.inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_MULTI_LINE or
            InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS or
            if (directLatinInput) InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD else InputType.TYPE_TEXT_VARIATION_NORMAL
        outAttrs.imeOptions = EditorInfo.IME_ACTION_NONE or EditorInfo.IME_FLAG_NO_EXTRACT_UI or
            EditorInfo.IME_FLAG_NO_FULLSCREEN or EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING
        outAttrs.initialSelStart = 0
        outAttrs.initialSelEnd = 0
        inputConnection?.cancelComposition()
        return TerminalInputConnection(this).also { inputConnection = it }
    }

    internal fun handleKey(event: KeyEvent): Boolean {
        if (event.action == KeyEvent.ACTION_MULTIPLE && event.characters != null) {
            input.commit(event.characters)
            return true
        }
        val unmodifiedMeta = event.metaState and (KeyEvent.META_CTRL_MASK or KeyEvent.META_ALT_MASK or KeyEvent.META_META_MASK).inv()
        val key = terminalKey(event.keyCode, event.getUnicodeChar(unmodifiedMeta)) ?: return false
        if (event.action == KeyEvent.ACTION_DOWN) {
            input.key(key, KeyModifiers(event.isShiftPressed, event.isCtrlPressed, event.isAltPressed, event.isMetaPressed))
        }
        return true // Releases are consumed but never sent to Rust.
    }

    override fun dispatchKeyEventPreIme(event: KeyEvent): Boolean {
        // Raw hardware/injected keys belong to the terminal, not to the IME's editor.
        // ViewRootImpl stops dispatch when this returns true, so the same key cannot also
        // become an IME commit and then reach onKeyDown as an unhandled fallback.
        // IME-originated events retain their normal InputConnection/post-IME route.
        if (event.flags and KeyEvent.FLAG_SOFT_KEYBOARD == 0 && handleKey(event)) return true
        return super.dispatchKeyEventPreIme(event)
    }

    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean = handleKey(event) || super.onKeyDown(keyCode, event)
    override fun onKeyUp(keyCode: Int, event: KeyEvent): Boolean = handleKey(event) || super.onKeyUp(keyCode, event)

    override fun performClick(): Boolean {
        super.performClick()
        return true
    }

    /** Changes the cell size: the grid is re-laid out and, unless [resize] is false, the session told. */
    fun setFontSize(sp: Float, resize: Boolean = true) {
        val clamped = sp.coerceIn(TerminalPrefs.MinFontSp, TerminalPrefs.MaxFontSp)
        if (clamped == fontSizeSp) return
        fontSizeSp = clamped
        textPaint.typeface = baseTypeface
        textPaint.isFakeBoldText = false
        textPaint.textSize = TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, clamped, resources.displayMetrics)
        cellWidth = ceil(textPaint.measureText("M"))
        cellHeight = ceil(textPaint.fontMetrics.bottom - textPaint.fontMetrics.top)
        baseline = -textPaint.fontMetrics.top
        glyphs.evictAll()
        scrollRemainder = 0f
        clearSelection()
        if (resize) resizeSession()
        invalidate()
    }

    override fun onTouchEvent(event: MotionEvent): Boolean {
        if (event.actionMasked == MotionEvent.ACTION_DOWN) pinchedSinceDown = false
        scaleGestures.onTouchEvent(event)
        if (pinching || scaleGestures.isInProgress) return true
        if (pinchedSinceDown) {
            // The gesture detector never saw this finger go down (it was cancelled at the pinch's
            // start), so everything up to the last finger lifting belongs to the pinch.
            if (event.actionMasked == MotionEvent.ACTION_UP || event.actionMasked == MotionEvent.ACTION_CANCEL) pinchedSinceDown = false
            return true
        }
        if (event.actionMasked == MotionEvent.ACTION_DOWN) parent?.requestDisallowInterceptTouchEvent(true)
        if (selection != null && event.actionMasked == MotionEvent.ACTION_MOVE) {
            position(event.x, event.y)?.let { selection?.end = it }
            invalidate()
        }
        return gestures.onTouchEvent(event) || event.actionMasked == MotionEvent.ACTION_UP
    }

    private fun position(x: Float, y: Float): CellPosition? =
        grid.position(x - horizontalInset, y, cellWidth, cellHeight, selection)

    fun beginSelection(position: CellPosition, word: Boolean = false) {
        if (!grid.hasGrid) return
        selection = if (word) {
            TerminalSelection.word(selection?.rows ?: grid.rows, selection?.columns ?: grid.columns, position)
        } else {
            TerminalSelection(grid.rows, grid.columns, position)
        }
        scroller.forceFinished(true)
        onSelectionChanged()
        invalidate()
    }

    fun clearSelection() {
        selection = null
        onSelectionChanged()
        invalidate()
    }

    fun copySelection() {
        selection?.let {
            context.getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText("Terminal selection", it.text()))
        }
        clearSelection()
    }

    private fun scrollPixels(delta: Float) {
        scrollRemainder += delta
        val rows = (scrollRemainder / cellHeight).toInt()
        if (rows != 0) {
            scrollRows(rows)
            scrollRemainder -= rows * cellHeight
        }
    }

    /**
     * Scrolls what the user is looking at by [rows] (negative = up), never shell history
     * ([scrollRoute]): wheel events while the program tracks the mouse, the tmux or herdr target's
     * own history, else the local viewport (arrow keys on the alternate screen).
     */
    private fun scrollRows(rows: Int) {
        val targets = targetScroller
        when (scrollRoute(grid.modes, target)) {
            ScrollRoute.WHEEL -> sessionCall {
                scroll(ViewportScroll.Wheel(rows, scrollCell.column.toUShort(), scrollCell.row.toUShort()))
            }
            ScrollRoute.TMUX, ScrollRoute.HERDR ->
                if (targets != null) targets.scroll(rows) else sessionCall { scroll(ViewportScroll.Delta(rows)) }
            ScrollRoute.VIEWPORT -> sessionCall { scroll(ViewportScroll.Delta(rows)) }
        }
        updateScrolledAway()
    }

    /**
     * Lets a tmux or herdr [target] scroll its own history through [send] (`scroll_target`), one
     * call at a time in [scope].
     */
    fun useTargetScroll(target: TerminalTarget, scope: CoroutineScope, send: suspend (TargetScroll) -> Unit) {
        this.target = target
        targetScroller = TargetScroller(scope, send) { updateScrolledAway() }
    }

    /** Runs [send] at once, or after the target's `Bottom` while it is scrolled away (no typing into copy mode). */
    private fun atBottom(send: () -> Unit) {
        val targets = targetScroller
        if (targets == null) send() else targets.input(send)
    }

    private fun updateScrolledAway() {
        val away = scrollToBottomVisible(grid.scrollback, grid.modes, grid.rows.size, targetScroller?.away == true)
        if (away != reportedAway) {
            reportedAway = away
            onScrolledAwayChanged(away)
        }
    }

    /** One page up into the scrollback (the toolbar's history key), routed like a swipe. */
    fun pageUp() {
        clearSelection()
        scroller.forceFinished(true)
        val page = (grid.rows.size - 1).coerceAtLeast(1)
        scrollRows(-page)
    }

    /**
     * Back to the bottom (the scroll-to-bottom button, and holding the history key): the target's
     * live screen when it is scrolled away, the viewport's bottom when that is.
     */
    fun jumpToBottom() {
        clearSelection()
        scroller.forceFinished(true)
        scrollRemainder = 0f
        val targets = targetScroller
        val targetAway = targets?.away == true
        if (targetAway) targets?.bottom()
        if (!targetAway || viewportAway(grid.scrollback, grid.modes, grid.rows.size)) sessionCall { scroll(ViewportScroll.Bottom) }
        updateScrolledAway()
    }

    override fun computeScroll() {
        if (scroller.computeScrollOffset()) {
            scrollPixels((scroller.currY - flingY).toFloat())
            flingY = scroller.currY
            postInvalidateOnAnimation()
        }
    }

    override fun onAttachedToWindow() {
        super.onAttachedToWindow()
        postDelayed(blink, 500)
        frameReady() // Also drain an event that arrived before attachment.
    }

    override fun onWindowVisibilityChanged(visibility: Int) {
        super.onWindowVisibilityChanged(visibility)
        // A retained view shown again (the app returned from the background): its next draw counts
        // even when the terminal's content did not change meanwhile. A view that has not had its
        // first frame yet waits for it instead (a blank draw is not the terminal on screen).
        if (visibility == VISIBLE && grid.hasGrid) {
            appliedFrames.shown()
            invalidate()
        }
    }

    override fun onDetachedFromWindow() {
        removeCallbacks(blink)
        scroller.forceFinished(true)
        inputConnection?.cancelComposition()
        inputConnection = null
        input.discardComposition()
        Choreographer.getInstance().removeFrameCallback(frameCallback)
        framePending = false
        super.onDetachedFromWindow()
    }

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        super.onSizeChanged(w, h, oldw, oldh)
        clearSelection()
        resizeSession()
    }

    /** The grid the view's current size and cell metrics give, inside the horizontal inset. */
    internal fun currentGridSize(): GridSize? = gridSize((width - 2 * horizontalInset).toInt(), height, cellWidth, cellHeight)

    private fun resizeSession() {
        val size = currentGridSize() ?: return
        if (size != lastSize && sessionCall { resize(size.columns, size.rows) }) lastSize = size
    }

    private fun UInt.opaque() = toInt() or (0xff shl 24)

    override fun onDraw(canvas: Canvas) {
        val start = System.nanoTime()
        // AndroidView may inherit a larger Compose canvas clip. Bound every draw, including
        // old grids during resize, cursor, selection and composing text, to this View.
        val checkpoint = canvas.save()
        canvas.clipRect(0f, 0f, width.toFloat(), height.toFloat())
        paint.color = grid.background.opaque()
        canvas.drawRect(0f, 0f, width.toFloat(), height.toFloat(), paint) // Includes grid margins.
        canvas.save()
        canvas.translate(horizontalInset, 0f)
        (selection?.rows ?: grid.rows).forEachIndexed { rowIndex, row ->
            val y = rowIndex * cellHeight
            row.cells.forEachIndexed { column, cell ->
                paint.color = cell.style.background.opaque()
                canvas.drawRect(column * cellWidth, y, (column + 1) * cellWidth, y + cellHeight, paint)
            }
            row.cells.forEachIndexed { column, cell ->
                if (cell.width != CellWidth.SPACER_TAIL) drawCell(canvas, column * cellWidth, y, cell)
            }
        }
        grid.cursor?.takeIf { selection == null }?.let { cursor ->
            if (!cursor.blinking || cursorVisible) {
                val x = cursor.column.toInt() * cellWidth
                val y = cursor.row.toInt() * cellHeight
                val w = cellWidth * if (cursor.wide) 2 else 1
                paint.color = cursor.color.opaque()
                paint.style = Paint.Style.FILL
                when (cursor.shape) {
                    CursorShape.BAR -> canvas.drawRect(x, y, x + 2 * resources.displayMetrics.density, y + cellHeight, paint)
                    CursorShape.UNDERLINE -> canvas.drawRect(x, y + cellHeight - 2, x + w, y + cellHeight, paint)
                    CursorShape.BLOCK_HOLLOW -> {
                        paint.style = Paint.Style.STROKE
                        paint.strokeWidth = 2f
                        canvas.drawRect(x + 1, y + 1, x + w - 1, y + cellHeight - 1, paint)
                        paint.style = Paint.Style.FILL
                    }
                    CursorShape.BLOCK -> {
                        canvas.drawRect(x, y, x + w, y + cellHeight, paint)
                        grid.rows.getOrNull(cursor.row.toInt())?.cells?.getOrNull(cursor.column.toInt())?.let { cell ->
                            drawCell(canvas, x, y, cell.copy(style = cell.style.copy(foreground = cell.style.background)))
                        }
                    }
                }
            }
        }
        selection?.let { selected ->
            paint.color = Or2Colors.Accent.copy(alpha = 0.35f).toArgb()
            selected.rows.indices.forEach { row ->
                selected.range(row)?.let { range ->
                    canvas.drawRect(range.first * cellWidth, row * cellHeight,
                        (range.last + 1) * cellWidth, (row + 1) * cellHeight, paint)
                }
            }
        }
        if (input.composing.isNotEmpty() && selection == null) {
            grid.cursor?.let { cursor ->
                val x = cursor.column.toInt() * cellWidth
                val y = cursor.row.toInt() * cellHeight
                textPaint.typeface = baseTypeface
                textPaint.isFakeBoldText = false
                textPaint.color = Or2Colors.Text.toArgb()
                textPaint.alpha = 255
                val w = textPaint.measureText(input.composing).coerceAtLeast(compositionCells(input.composing) * cellWidth)
                paint.color = Or2Colors.AccentMuted.toArgb()
                canvas.drawRect(x, y, x + w, y + cellHeight, paint)
                canvas.drawText(input.composing, x, y + baseline, textPaint)
                paint.color = Or2Colors.Accent.toArgb()
                canvas.drawRect(x, y + cellHeight - 2, x + w, y + cellHeight, paint)
            }
        }
        canvas.restore()
        drawScrollIndicator(canvas)
        drawTimings.record(System.nanoTime() - start)
        if (showTimings) {
            paint.color = 0xdd000000.toInt()
            canvas.drawRect(0f, height - cellHeight, width.toFloat(), height.toFloat(), paint)
            textPaint.color = Or2Colors.Text.toArgb()
            textPaint.typeface = baseTypeface
            textPaint.isFakeBoldText = false
            textPaint.alpha = 255
            canvas.drawText("apply p95 %.2f · draw p95 %.2f ms".format(applyTimings.percentile(95), drawTimings.percentile(95)),
                0f, height - cellHeight + baseline, textPaint)
        }
        canvas.restoreToCount(checkpoint)
        // Reported once this frame has really been committed, not when it was only asked for.
        if (appliedFrames.drawn()) viewTreeObserver.registerFrameCommitCallback { onFrameDrawn() }
    }

    /** A thin accent bar on the right edge showing where the viewport sits in the scrollback. */
    private fun drawScrollIndicator(canvas: Canvas) {
        val total = grid.scrollback.totalRows.toFloat()
        val visible = grid.rows.size.toFloat()
        if (visible <= 0f || total <= visible || height <= 0) return
        val density = resources.displayMetrics.density
        val barHeight = (height * visible / total).coerceAtLeast(24 * density)
        val top = (height - barHeight) * (grid.scrollback.offset.toFloat() / (total - visible)).coerceIn(0f, 1f)
        val right = width - density
        paint.style = Paint.Style.FILL
        paint.color = Or2Colors.Accent.copy(alpha = 0.85f).toArgb()
        canvas.drawRoundRect(right - 3 * density, top, right, top + barHeight, 1.5f * density, 1.5f * density, paint)
    }

    private fun drawCell(canvas: Canvas, x: Float, y: Float, cell: ResolvedCell) {
        val w = cellWidth * if (cell.width == CellWidth.WIDE) 2 else 1
        if (cell.text.isNotEmpty()) {
            val typeface = typefaces[(if (cell.style.bold) Typeface.BOLD else 0) or
                (if (cell.style.italic) Typeface.ITALIC else 0)]
            val fakeBold = cell.style.bold && !typeface.isBold
            val key = Glyph(cell.text, cell.width == CellWidth.WIDE, cell.style.foreground,
                cell.style.bold, cell.style.italic, cell.style.faint, fakeBold)
            val picture = glyphs[key] ?: Picture().also { picture ->
                val glyphCanvas = picture.beginRecording(ceil(w).toInt(), ceil(cellHeight).toInt())
                glyphCanvas.clipRect(0f, 0f, w, cellHeight)
                textPaint.typeface = typeface
                textPaint.isFakeBoldText = fakeBold
                textPaint.color = cell.style.foreground.opaque()
                textPaint.alpha = if (cell.style.faint) 128 else 255
                val measured = textPaint.measureText(cell.text)
                glyphCanvas.save()
                if (measured > w) glyphCanvas.scale(w / measured, 1f)
                glyphCanvas.drawText(cell.text, ((w - measured) / 2).coerceAtLeast(0f), baseline, textPaint)
                glyphCanvas.restore()
                picture.endRecording()
                glyphs.put(key, picture)
            }
            canvas.save()
            canvas.translate(x, y)
            canvas.drawPicture(picture)
            canvas.restore()
        }
        if (cell.style.underline != Underline.NONE || cell.style.strikethrough || cell.style.overline) {
            canvas.save()
            canvas.translate(x, y)
            canvas.clipRect(0f, 0f, w, cellHeight)
            decorations(canvas, w, cell.style)
            canvas.restore()
        }
    }

    private fun decorations(canvas: Canvas, w: Float, style: CellStyle) {
        paint.style = Paint.Style.STROKE
        paint.strokeWidth = resources.displayMetrics.density
        paint.color = style.foreground.opaque()
        paint.alpha = if (style.faint) 128 else 255
        if (style.strikethrough) canvas.drawLine(0f, baseline * .6f, w, baseline * .6f, paint)
        if (style.overline) canvas.drawLine(0f, paint.strokeWidth, w, paint.strokeWidth, paint)
        paint.color = (style.underlineColor ?: style.foreground).opaque()
        paint.alpha = if (style.faint) 128 else 255
        val y = cellHeight - paint.strokeWidth * 2
        when (style.underline) {
            Underline.NONE -> Unit
            Underline.SINGLE -> canvas.drawLine(0f, y, w, y, paint)
            Underline.DOUBLE -> {
                canvas.drawLine(0f, y - paint.strokeWidth * 2, w, y - paint.strokeWidth * 2, paint)
                canvas.drawLine(0f, y, w, y, paint)
            }
            Underline.CURLY -> {
                val path = Path().apply {
                    moveTo(0f, y)
                    var x = 0f
                    val step = paint.strokeWidth * 2
                    while (x < w) {
                        quadTo(x + step / 2, y - step, x + step, y)
                        quadTo(x + step * 1.5f, y + step, x + step * 2, y)
                        x += step * 2
                    }
                }
                canvas.drawPath(path, paint)
            }
            Underline.DOTTED, Underline.DASHED -> {
                val dash = paint.strokeWidth * if (style.underline == Underline.DOTTED) 1 else 3
                var x = 0f
                while (x < w) {
                    canvas.drawLine(x, y, (x + dash).coerceAtMost(w), y, paint)
                    x += dash + paint.strokeWidth * 2
                }
            }
        }
        paint.style = Paint.Style.FILL
        paint.alpha = 255
    }
}
