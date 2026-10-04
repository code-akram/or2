package io.github.code_akram.or2.terminal

import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Point
import android.graphics.Rect
import android.graphics.Typeface
import android.os.Bundle
import android.os.SystemClock
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.github.code_akram.or2.ffi.CursorShape
import io.github.code_akram.or2.ffi.TerminalCursor
import io.github.code_akram.or2.ffi.CellStyle
import io.github.code_akram.or2.ffi.CellWidth
import io.github.code_akram.or2.ffi.TerminalCell
import io.github.code_akram.or2.ffi.TerminalRow
import io.github.code_akram.or2.ffi.Underline
import java.io.File
import io.github.code_akram.or2.ui.Or2Dimens
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

/** Captures only synthetic terminal content, under the target app's private files directory. */
@RunWith(AndroidJUnit4::class)
class TerminalVisualDeviceTest {
    private val instrumentation get() = InstrumentationRegistry.getInstrumentation()

    @Test fun batchedAsciiKeepsTheLegacyCellOriginsBackgroundsAndFaintAlpha() {
        instrumentation.runOnMainSync {
            val view = TerminalView(instrumentation.targetContext)
            val columns = 56
            val style = CellStyle(0xc0caf5u, DefaultBackground, null, Underline.NONE,
                false, false, false, false, false)
            val styles = listOf(style, style.copy(background = 0x334455u), style.copy(faint = true))
            val frame = terminalVisualFrame(columns.toUShort(), 3u, CursorShape.BAR).copy(
                cursor = null, background = DefaultBackground, styles = styles,
                scrollback = io.github.code_akram.or2.ffi.Scrollback(3u, 0u),
                changedRows = List(3) { row -> TerminalRow(row.toUShort(), false,
                    List(columns) { column -> TerminalCell("MiW.fg"[column % 6].toString(), CellWidth.NARROW,
                        (if (row == 0) 0 else if (row == 1 && column in 10..30) 1 else 2).toUInt()) }) })
            val width = (view.horizontalInset * 2 + columns * view.cellWidth).toInt()
            val height = (3 * view.cellHeight).toInt()
            view.layout(0, 0, width, height)
            view.grid.apply(frame)
            assertTrue("The ordinary fixture must exercise drawGlyphs", view.grid.rows[0].cells.any { view.canBatchGlyph(it) })
            val ordinary = ResolvedCell("M", CellWidth.NARROW, style)
            assertFalse(view.canBatchGlyph(ordinary.copy(width = CellWidth.WIDE)))
            assertFalse(view.canBatchGlyph(ordinary.copy(text = "e\u0301")))
            assertFalse(view.canBatchGlyph(ordinary.copy(text = "\u0301")))
            assertFalse(view.canBatchGlyph(ordinary.copy(style = style.copy(italic = true))))
            assertFalse(view.canBatchGlyph(ordinary.copy(text = "😀")))
            if (view.boldUsesFake) assertFalse(view.canBatchGlyph(ordinary.copy(style = style.copy(bold = true))))
            val actual = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888)
            val expected = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888)
            try {
                view.draw(Canvas(actual))
                val canvas = Canvas(expected)
                canvas.drawColor(DefaultBackground.toInt() or (0xff shl 24))
                val text = Paint(Paint.ANTI_ALIAS_FLAG).apply {
                    textSize = view.fontSizeSp * view.resources.displayMetrics.density
                    typeface = terminalTypeface(this)
                }
                val background = Paint()
                val baseline = -text.fontMetrics.top
                canvas.translate(view.horizontalInset, 0f)
                frame.changedRows.forEach { row ->
                    row.cells.forEachIndexed { column, cell ->
                        val cellStyle = styles[cell.style.toInt()]
                        val x = column * view.cellWidth
                        val y = row.index.toInt() * view.cellHeight
                        background.color = cellStyle.background.toInt() or (0xff shl 24)
                        canvas.drawRect(x, y, x + view.cellWidth, y + view.cellHeight, background)
                        text.color = cellStyle.foreground.toInt() or (0xff shl 24)
                        text.alpha = if (cellStyle.faint) 128 else 255
                        val measured = text.measureText(cell.text)
                        canvas.save()
                        canvas.translate(x, y)
                        canvas.clipRect(0f, 0f, view.cellWidth, view.cellHeight)
                        if (measured > view.cellWidth) canvas.scale(view.cellWidth / measured, 1f)
                        canvas.drawText(cell.text, ((view.cellWidth - measured) / 2).coerceAtLeast(0f), baseline, text)
                        canvas.restore()
                    }
                }
                // Explicit glyph positions should produce the same raster on a software Canvas.
                // Hardware equivalence is also reviewed with the existing captured style fixtures.
                assertTrue("Batched text drifted from per-cell rendering", actual.sameAs(expected))
            } finally {
                actual.recycle()
                expected.recycle()
            }
        }
    }

    private fun await(scenario: ActivityScenario<TerminalProbeActivity>, predicate: (TerminalProbeActivity, TerminalView) -> Boolean) {
        val deadline = SystemClock.uptimeMillis() + 8_000
        while (SystemClock.uptimeMillis() < deadline) {
            var ready = false
            scenario.onActivity { activity -> ready = activity.terminalView()?.let { predicate(activity, it) } == true }
            if (ready) return
            SystemClock.sleep(20)
        }
        fail("Terminal frame or Window metrics did not arrive")
    }

    /**
     * Every toolbar key is laid out inside the pill, in order and not overlapping: nothing needs a scroll, with or without
     * a selection (Copy and Clear then take the typing keys' place).
     */
    private fun assertToolbarKeysFitWithoutScrolling(view: TerminalView) {
        val toolbar = checkNotNull(view.toolbarBounds) { "Key toolbar not laid out" }
        assertTrue("Toolbar must be visible", toolbar.width() > 0 && toolbar.height() > 0)
        val density = view.resources.displayMetrics.density
        val labels = toolbarKeys(selecting = view.selection != null).map { it.tag } + ToolbarToggles.map { it.tag }
        assertTrue("Toolbar keys $labels must all be laid out, found ${view.toolbarKeyBounds.keys}", view.toolbarKeyBounds.keys.containsAll(labels))
        var right = toolbar.left
        labels.forEach { label ->
            val bounds = view.toolbarKeyBounds.getValue(label)
            assertTrue("$label must be visible", bounds.width() > 0 && bounds.height() > 0)
            assertTrue("$label ($bounds) must fit within $toolbar without scrolling", toolbar.contains(bounds))
            assertTrue("$label touch box must be at least 32 dp wide", bounds.width() >= Or2Dimens.KeyTouchWidth.value * density - 1)
            assertTrue("$label touch box must be at least 40 dp tall (the key itself is drawn 30 dp)", bounds.height() >= 40 * density - 1)
            assertTrue("$label must follow the preceding key", bounds.left >= right - 1)
            right = bounds.right
        }
    }

    private fun capture(scenario: ActivityScenario<TerminalProbeActivity>, name: String) {
        // Settle Compose key state and hardware render submission before asking SurfaceFlinger.
        instrumentation.waitForIdleSync()
        SystemClock.sleep(100)
        val bounds = Rect()
        var compositionEdge: Point? = null
        scenario.onActivity { activity ->
            activity.window.decorView.getWindowVisibleDisplayFrame(bounds)
            val view = activity.terminalView()!!
            assertToolbarKeysFitWithoutScrolling(view) // Direct layout reads stay on the UI thread.
            val position = IntArray(2)
            view.getLocationOnScreen(position)
            bounds.top = position[1] // Exclude system status and debug toolbar; retain terminal + keys.
            if (name == "composition-armed-keys") {
                val cursor = view.grid.cursor!!
                // e + combining acute is one cell, CJK and emoji each occupy two.
                compositionEdge = Point(position[0] - bounds.left + (view.horizontalInset + (cursor.column.toInt() + 5) * view.cellWidth).toInt() - 1,
                    ((cursor.row.toInt() + 1) * view.cellHeight).toInt() - 1)
            }
        }
        val screenshot = instrumentation.uiAutomation.takeScreenshot()
        assertNotNull("Hardware screenshot unavailable", screenshot)
        val cropped = Bitmap.createBitmap(screenshot!!, bounds.left, bounds.top,
            bounds.width().coerceAtMost(screenshot.width - bounds.left), bounds.height().coerceAtMost(screenshot.height - bounds.top))
        val directory = File(instrumentation.targetContext.filesDir, "terminal-review").apply { mkdirs() }
        File(directory, "$name.png").outputStream().use { assertTrue(cropped.compress(Bitmap.CompressFormat.PNG, 100, it)) }
        compositionEdge?.let { edge ->
            assertEquals("Composition underline must cover all five cells", 0xff89b4fa.toInt(), cropped.getPixel(edge.x, edge.y))
        }
        cropped.recycle()
        screenshot.recycle()
    }

    @Test fun capturesProbeStylesWideCursorsCompositionKeysAndSelection() {
        ActivityScenario.launch(TerminalProbeActivity::class.java).use { scenario ->
            await(scenario) { _, view -> view.grid.hasGrid && view.grid.rows.size > 12 }
            scenario.onActivity { activity ->
                val file = File("/system/fonts/DroidSansMono.ttf")
                if (file.isFile) {
                    val probe = Paint().apply { textSize = 30f; typeface = Typeface.Builder(file).build() }
                    if (probe.hasMonospacedAdvances()) {
                        val view = activity.terminalView()!!
                        assertTrue("Validated system font must be used", view.fontHasMonospacedAdvances)
                        assertTrue("DroidSansMono needs synthetic bold", view.boldUsesFake)
                    }
                }
                assertEquals(0, compositionCells(""))
                assertEquals(1, compositionCells("e\u0301"))
                assertEquals(5, compositionCells("e\u0301界😀"))
                assertEquals(2, compositionCells("👩‍💻"))
                assertEquals(2, compositionCells("🇴🇲"))
                assertEquals(2, compositionCells("1\uFE0F\u20E3"))
                assertEquals(1, compositionCells("©\uFE0E"))
            }
            capture(scenario, "probe")
            CursorShape.entries.forEach { shape ->
                scenario.onActivity { activity ->
                    val view = activity.terminalView()!!
                    activity.display(terminalVisualFrame(view.grid.columns.toUShort(), view.grid.rows.size.toUShort(), shape))
                }
                capture(scenario, "styles-${shape.name.lowercase()}")
            }
            scenario.onActivity { activity ->
                val view = activity.terminalView()!!
                view.input.compose("e\u0301界😀")
                view.input.toggleCtrl()
                view.input.toggleAlt()
            }
            capture(scenario, "composition-armed-keys")
            scenario.onActivity { activity ->
                val view = activity.terminalView()!!
                view.input.discardComposition()
                view.beginSelection(CellPosition(2, 11))
                view.selection!!.end = CellPosition(5, 11)
                view.invalidate()
                assertEquals("界😀e\u0301", view.selection!!.text())
            }
            capture(scenario, "selection")
        }
    }

    @Test fun terminalCannotPaintOverComposeHeaderOrOutsideItsSideMargins() {
        ActivityScenario.launch(TerminalProbeActivity::class.java).use { scenario ->
            await(scenario) { _, view -> view.grid.hasGrid }
            var oldWidth = 0
            scenario.onActivity { activity ->
                oldWidth = activity.terminalView()!!.width
                activity.showBoundsFixture()
            }
            await(scenario) { _, view ->
                val size = view.currentGridSize()
                size != null && view.width < oldWidth && view.grid.hasGrid &&
                    view.grid.columns == size.columns.toInt() && view.grid.rows.size == size.rows.toInt()
            }
            for (selecting in listOf(false, true)) {
                val outside = mutableListOf<Point>()
                var inside = Point()
                var oldDraws = 0
                scenario.onActivity { activity ->
                    val view = activity.terminalView()!!
                    oldDraws = view.drawTimings.count
                    val size = view.currentGridSize()!!
                    val columns = size.columns.toInt()
                    val rows = size.rows.toInt()
                    // A retained grid can temporarily exceed the viewport during resize.
                    activity.display(terminalVisualFrame((columns + 4).toUShort(), (rows + 4).toUShort(), CursorShape.BAR)
                        .copy(cursor = TerminalCursor((columns - 1).toUShort(), 1u, true, CursorShape.BLOCK, false, 0x89b4fau)))
                    if (selecting) {
                        view.beginSelection(CellPosition(0, 0))
                        view.selection!!.end = CellPosition(columns + 3, rows + 3)
                        view.invalidate()
                    } else {
                        view.input.compose("界😀".repeat(20)) // Must not escape past the right edge.
                    }
                    val position = IntArray(2)
                    view.getLocationOnScreen(position)
                    val inset = (8 * view.resources.displayMetrics.density).toInt()
                    val y = position[1] + (view.cellHeight * 1.5f).toInt()
                    outside += Point(position[0] + view.width / 2, position[1] - inset) // Compose header.
                    outside += Point(position[0] - inset, y)
                    outside += Point(position[0] + view.width + inset, y)
                    inside = Point(position[0] + view.width / 2, position[1] + view.height / 2)
                }
                await(scenario) { _, view -> view.drawTimings.count > oldDraws }
                instrumentation.waitForIdleSync()
                SystemClock.sleep(100)
                val screenshot = instrumentation.uiAutomation.takeScreenshot()
                assertNotNull(screenshot)
                val directory = File(instrumentation.targetContext.filesDir, "terminal-review").apply { mkdirs() }
                val name = if (selecting) "bounds-selection" else "bounds-composition"
                File(directory, "$name.png").outputStream().use {
                    assertTrue(screenshot!!.compress(Bitmap.CompressFormat.PNG, 100, it))
                }
                try {
                    outside.forEach { point ->
                        assertEquals("$name painted outside terminal at $point", 0xff336699.toInt(), screenshot!!.getPixel(point.x, point.y))
                    }
                    assertNotEquals("Terminal itself must still be drawn", 0xff336699.toInt(), screenshot!!.getPixel(inside.x, inside.y))
                } finally {
                    screenshot!!.recycle()
                }
            }
        }
    }

    @Test fun measuresNativeProbeAndDenseFullScreenUpdatesWithWindowMetrics() {
        val reports = mutableListOf<String>()
        ActivityScenario.launch(TerminalProbeActivity::class.java).use { scenario ->
            await(scenario) { activity, view -> view.grid.hasGrid && activity.renderTimings.count > 0 }
            fun sample(dense: Boolean) {
                var oldMetrics = 0
                var oldDraws = 0
                var oldSequence = 0uL
                scenario.onActivity { activity ->
                    oldMetrics = activity.renderTimings.count
                    val view = activity.terminalView()!!
                    oldDraws = view.drawTimings.count
                    oldSequence = view.grid.sequence
                    if (dense) activity.fullScreenUpdate() else activity.requestProbeFrame()
                }
                await(scenario) { activity, view ->
                    activity.renderTimings.count > oldMetrics && view.drawTimings.count > oldDraws &&
                        (dense || view.grid.sequence > oldSequence)
                }
            }
            for (dense in listOf(false, true)) {
                repeat(10) { sample(dense) } // Warm glyph cache and render pipeline.
                scenario.onActivity { it.resetTimings() }
                repeat(60) { sample(dense) }
                scenario.onActivity { activity ->
                    val view = activity.terminalView()!!
                    assertTrue(view.isHardwareAccelerated)
                    assertTrue(view.applyTimings.count >= 60)
                    assertTrue(view.drawTimings.count >= 60)
                    assertTrue(activity.renderTimings.count >= 60)
                    assertTrue(activity.renderTimings.percentile(95) > 0)
                    val workload = if (dense) "Dense synthetic full-screen grid (Kotlin merge; excludes FFI)" else "Native probe full snapshots (takeFrame + merge; includes FFI)"
                    reports += "$workload ${view.grid.columns}x${view.grid.rows.size}\n${activity.timingReport()}"
                }
            }
        }
        val report = reports.joinToString("\n\n")
        val directory = File(instrumentation.targetContext.filesDir, "terminal-review").apply { mkdirs() }
        File(directory, "timings.txt").writeText(report)
        instrumentation.sendStatus(0, Bundle().apply { putString("terminal_frame_timings", report) })
    }
}
