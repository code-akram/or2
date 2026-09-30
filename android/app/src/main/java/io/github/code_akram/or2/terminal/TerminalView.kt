package io.github.code_akram.or2.terminal

import android.content.Context
import android.content.ClipData
import android.content.ClipboardManager
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Path
import android.graphics.Picture
import android.graphics.Typeface
import android.text.InputType
import android.util.LruCache
import android.util.TypedValue
import android.view.Choreographer
import android.view.GestureDetector
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.View
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputMethodManager
import android.widget.OverScroller
import io.github.code_akram.or2.ffi.CellStyle
import io.github.code_akram.or2.ffi.CellWidth
import io.github.code_akram.or2.ffi.CursorShape
import io.github.code_akram.or2.ffi.KeyModifiers
import io.github.code_akram.or2.ffi.SessionException
import io.github.code_akram.or2.ffi.SessionInterface
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.Underline
import io.github.code_akram.or2.ffi.ViewportScroll
import kotlin.math.ceil

/** Canvas is the only renderer. The cache retains glyph commands, not terminal bitmaps. */
class TerminalView(context: Context) : View(context) {
    val grid = TerminalGrid()
    val applyTimings = FrameTimings()
    val drawTimings = FrameTimings()
    var showTimings = false
    private val paint = Paint(Paint.ANTI_ALIAS_FLAG)
    private val textPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        typeface = Typeface.MONOSPACE
        textSize = TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_SP, 14f, resources.displayMetrics)
    }
    val cellWidth = ceil(textPaint.measureText("M"))
    val cellHeight = ceil(textPaint.fontMetrics.bottom - textPaint.fontMetrics.top)
    private val baseline = -textPaint.fontMetrics.top
    private data class Glyph(
        val text: String, val wide: Boolean, val foreground: UInt,
        val bold: Boolean, val italic: Boolean, val faint: Boolean,
    )
    private val glyphs = LruCache<Glyph, Picture>(2048)
    var onInputChanged: () -> Unit = {}
    var onSelectionChanged: () -> Unit = {}
    val input = TerminalInput(
        { text -> sessionCall { sendText(text) } },
        { key -> sessionCall { sendKey(key) } },
        { invalidate(); onInputChanged() },
    )
    var selection: TerminalSelection? = null
        private set
    private val scroller = OverScroller(context)
    private var flingY = 0
    private var scrollRemainder = 0f
    private val gestures = GestureDetector(context, object : GestureDetector.SimpleOnGestureListener() {
        override fun onDown(e: MotionEvent): Boolean {
            scroller.forceFinished(true)
            return true
        }
        override fun onSingleTapUp(e: MotionEvent): Boolean {
            clearSelection()
            performClick()
            showKeyboard()
            return true
        }
        override fun onLongPress(e: MotionEvent) {
            beginSelection(position(e.x, e.y) ?: return)
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
    private var session: SessionInterface? = null
    private var connected = false
    private var lastSize: GridSize? = null
    private var framePending = false
    private var requestedFull = false
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
        session?.takeFrame()?.let { frame ->
            if (grid.apply(frame)) {
                requestedFull = false
                cursorVisible = true
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
        check(this.session == null || this.session === session) { "A terminal view belongs to one session" }
        this.session = session
        resizeSession()
    }

    fun sessionState(state: SessionState) {
        connected = state == SessionState.Connected
        if (connected && !grid.hasGrid) requestSnapshot()
    }

    private fun requestSnapshot() {
        if (!connected || requestedFull) return
        if (sessionCall { requestFullFrame() }) requestedFull = true
    }

    fun frameReady() {
        if (!framePending && isAttachedToWindow) {
            framePending = true
            Choreographer.getInstance().postFrameCallback(frameCallback)
        }
    }

    internal fun sessionCall(block: SessionInterface.() -> Unit): Boolean {
        val handle = session ?: return false
        return try {
            handle.block()
            true
        } catch (_: SessionException.NotConnected) {
            false
        } catch (_: SessionException.Closed) {
            false
        } catch (_: SessionException.InvalidKey) {
            false
        }
    }

    fun showKeyboard() {
        requestFocus()
        context.getSystemService(InputMethodManager::class.java).showSoftInput(this, InputMethodManager.SHOW_IMPLICIT)
    }

    override fun onCheckIsTextEditor() = true

    override fun onCreateInputConnection(outAttrs: EditorInfo): InputConnection {
        // Text + multiline retains CJK/dead-key composition. NO_SUGGESTIONS and omission of
        // AUTO_CORRECT prevent command rewriting; password types would break some IMEs.
        outAttrs.inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_MULTI_LINE or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
        outAttrs.imeOptions = EditorInfo.IME_ACTION_NONE or EditorInfo.IME_FLAG_NO_EXTRACT_UI or
            EditorInfo.IME_FLAG_NO_FULLSCREEN or EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING
        outAttrs.initialSelStart = 0
        outAttrs.initialSelEnd = 0
        return TerminalInputConnection(this)
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

    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean = handleKey(event) || super.onKeyDown(keyCode, event)
    override fun onKeyUp(keyCode: Int, event: KeyEvent): Boolean = handleKey(event) || super.onKeyUp(keyCode, event)

    override fun performClick(): Boolean {
        super.performClick()
        return true
    }

    override fun onTouchEvent(event: MotionEvent): Boolean {
        if (event.actionMasked == MotionEvent.ACTION_DOWN) parent?.requestDisallowInterceptTouchEvent(true)
        if (selection != null && event.actionMasked == MotionEvent.ACTION_MOVE) {
            position(event.x, event.y)?.let { selection?.end = it }
            invalidate()
        }
        return gestures.onTouchEvent(event) || event.actionMasked == MotionEvent.ACTION_UP
    }

    private fun position(x: Float, y: Float): CellPosition? {
        if (!grid.hasGrid) return null
        return CellPosition((x / cellWidth).toInt().coerceIn(0, grid.columns - 1),
            (y / cellHeight).toInt().coerceIn(0, grid.rows.lastIndex))
    }

    fun beginSelection(position: CellPosition) {
        if (!grid.hasGrid) return
        selection = TerminalSelection(grid.rows, grid.columns, position)
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
            sessionCall { scroll(ViewportScroll.Delta(rows)) }
            scrollRemainder -= rows * cellHeight
        }
    }

    fun jumpToBottom() {
        clearSelection()
        scroller.forceFinished(true)
        scrollRemainder = 0f
        sessionCall { scroll(ViewportScroll.Bottom) }
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

    override fun onDetachedFromWindow() {
        removeCallbacks(blink)
        scroller.forceFinished(true)
        input.finishComposition()
        Choreographer.getInstance().removeFrameCallback(frameCallback)
        framePending = false
        super.onDetachedFromWindow()
    }

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        super.onSizeChanged(w, h, oldw, oldh)
        clearSelection()
        resizeSession()
    }

    private fun resizeSession() {
        val size = gridSize(width, height, cellWidth, cellHeight) ?: return
        if (size != lastSize && sessionCall { resize(size.columns, size.rows) }) lastSize = size
    }

    private fun UInt.opaque() = toInt() or (0xff shl 24)

    override fun onDraw(canvas: Canvas) {
        val start = System.nanoTime()
        canvas.drawColor(grid.background.opaque()) // Includes right/bottom margins.
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
            paint.color = 0x663399ff
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
                textPaint.typeface = Typeface.MONOSPACE
                textPaint.color = android.graphics.Color.WHITE
                textPaint.alpha = 255
                val w = textPaint.measureText(input.composing)
                paint.color = 0xff23405b.toInt()
                canvas.drawRect(x, y, x + w, y + cellHeight, paint)
                canvas.drawText(input.composing, x, y + baseline, textPaint)
                paint.color = 0xff66ccff.toInt()
                canvas.drawRect(x, y + cellHeight - 2, x + w, y + cellHeight, paint)
            }
        }
        drawTimings.record(System.nanoTime() - start)
        if (showTimings) {
            paint.color = 0xdd000000.toInt()
            canvas.drawRect(0f, height - cellHeight, width.toFloat(), height.toFloat(), paint)
            textPaint.color = android.graphics.Color.WHITE
            textPaint.typeface = Typeface.MONOSPACE
            textPaint.alpha = 255
            canvas.drawText("apply p95 %.2f · draw p95 %.2f ms".format(applyTimings.percentile(95), drawTimings.percentile(95)),
                0f, height - cellHeight + baseline, textPaint)
        }
    }

    private fun drawCell(canvas: Canvas, x: Float, y: Float, cell: ResolvedCell) {
        val w = cellWidth * if (cell.width == CellWidth.WIDE) 2 else 1
        if (cell.text.isNotEmpty()) {
            val key = Glyph(cell.text, cell.width == CellWidth.WIDE, cell.style.foreground,
                cell.style.bold, cell.style.italic, cell.style.faint)
            val picture = glyphs[key] ?: Picture().also { picture ->
                val glyphCanvas = picture.beginRecording(ceil(w).toInt(), ceil(cellHeight).toInt())
                glyphCanvas.clipRect(0f, 0f, w, cellHeight)
                textPaint.typeface = Typeface.create(Typeface.MONOSPACE, when {
                    cell.style.bold && cell.style.italic -> Typeface.BOLD_ITALIC
                    cell.style.bold -> Typeface.BOLD
                    cell.style.italic -> Typeface.ITALIC
                    else -> Typeface.NORMAL
                })
                textPaint.color = cell.style.foreground.opaque()
                textPaint.alpha = if (cell.style.faint) 128 else 255
                val measured = textPaint.measureText(cell.text)
                glyphCanvas.save()
                if (measured > w) glyphCanvas.scale(w / measured, 1f)
                glyphCanvas.drawText(cell.text, 0f, baseline, textPaint)
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
