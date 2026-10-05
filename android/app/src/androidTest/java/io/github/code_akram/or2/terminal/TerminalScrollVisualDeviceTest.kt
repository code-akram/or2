package io.github.code_akram.or2.terminal

import android.graphics.Bitmap
import android.graphics.Rect
import android.os.SystemClock
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.github.code_akram.or2.ffi.CellLink
import io.github.code_akram.or2.ffi.CellStyle
import io.github.code_akram.or2.ffi.CellWidth
import io.github.code_akram.or2.ffi.CursorShape
import io.github.code_akram.or2.ffi.Scrollback
import io.github.code_akram.or2.ffi.TerminalCell
import io.github.code_akram.or2.ffi.TerminalFrame
import io.github.code_akram.or2.ffi.TerminalRow
import io.github.code_akram.or2.ffi.TerminalRowMove
import io.github.code_akram.or2.ffi.Underline
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

/** Dynamic hardware raster checks, not just cache counters or static duplicate rows. */
@RunWith(AndroidJUnit4::class)
class TerminalScrollVisualDeviceTest {
    private val instrumentation get() = InstrumentationRegistry.getInstrumentation()

    private fun await(scenario: ActivityScenario<TerminalProbeActivity>, condition: (TerminalView) -> Boolean) {
        val deadline = SystemClock.uptimeMillis() + 8_000
        while (SystemClock.uptimeMillis() < deadline) {
            var ready = false
            scenario.onActivity { ready = condition(it.terminalView()!!) }
            if (ready) return
            SystemClock.sleep(20)
        }
        fail("Terminal did not settle")
    }

    private fun pixels(scenario: ActivityScenario<TerminalProbeActivity>, bounds: Rect): Bitmap {
        // Copy our own hardware surface, not SystemUI/heads-up notifications from the daily app.
        val result = Bitmap.createBitmap(bounds.width(), bounds.height(), Bitmap.Config.ARGB_8888)
        val done = java.util.concurrent.CountDownLatch(1)
        var status = -1
        scenario.onActivity { activity ->
            android.view.PixelCopy.request(activity.window, bounds, result, {
                status = it
                done.countDown()
            }, android.os.Handler(android.os.Looper.getMainLooper()))
        }
        assertTrue("Window pixel copy timed out", done.await(5, java.util.concurrent.TimeUnit.SECONDS))
        assertEquals("Window pixel copy failed", android.view.PixelCopy.SUCCESS, status)
        return result
    }

    private fun capture(scenario: ActivityScenario<TerminalProbeActivity>, change: (TerminalView) -> Unit): Bitmap {
        var draws = 0
        var bounds = Rect()
        scenario.onActivity {
            val view = it.terminalView()!!
            draws = view.drawTimings.count
            change(view)
            val xy = IntArray(2)
            view.getLocationInWindow(xy)
            bounds = Rect(xy[0], xy[1], xy[0] + view.width, xy[1] + view.height)
            view.invalidate()
        }
        await(scenario) { it.drawTimings.count > draws }
        instrumentation.waitForIdleSync()
        SystemClock.sleep(100)
        return pixels(scenario, bounds)
    }

    @Test fun richRowsKeepExactPixelsWhileMovingEditingAndEvicting() {
        ActivityScenario.launch(TerminalProbeActivity::class.java).use { scenario ->
            await(scenario) { it.grid.hasGrid }
            lateinit var full: TerminalFrame
            scenario.onActivity { activity ->
                val view = activity.terminalView()!!
                val frame = terminalVisualFrame(view.grid.columns.toUShort(), view.grid.rows.size.toUShort(), CursorShape.BAR)
                val styles = listOf(
                    CellStyle(0xc0caf5u, DefaultBackground, null, Underline.NONE, false, false, false, false, false),
                    CellStyle(0x89b4fau, 0x203044u, null, Underline.SINGLE, true, false, false, false, false),
                    CellStyle(0xa6e3a1u, DefaultBackground, 0xf38ba8u, Underline.CURLY, false, true, true, true, true),
                )
                fun row(index: Int, number: Int): TerminalRow {
                    val cells = MutableList(view.grid.columns) { column ->
                        TerminalCell((('a'.code + (number + column) % 26).toChar()).toString(), CellWidth.NARROW, (column % 3).toUInt())
                    }
                    "%08d".format(number).forEachIndexed { column, c -> cells[column] = TerminalCell(c.toString(), CellWidth.NARROW, 0u) }
                    cells[10] = TerminalCell("界", CellWidth.WIDE, 1u)
                    cells[11] = TerminalCell("", CellWidth.SPACER_TAIL, 1u)
                    cells[14] = TerminalCell("😀", CellWidth.WIDE, 0u)
                    cells[15] = TerminalCell("", CellWidth.SPACER_TAIL, 0u)
                    cells[18] = TerminalCell("e\u0301", CellWidth.NARROW, 2u)
                    return TerminalRow(index.toUShort(), number % 2 == 0, cells,
                        listOf(CellLink(20u, 24u, "https://example.org/$number")))
                }
                full = frame.copy(sequence = 100u, cursor = null, background = DefaultBackground, styles = styles,
                    scrollback = Scrollback(view.grid.rows.size.toULong(), 0u),
                    changedRows = List(view.grid.rows.size) { row(it, it) })
                activity.display(full)
            }
            // Keep caching enabled across updates: clearing it before each snapshot masks reuse bugs.
            for (step in 0..15) {
                val actual = capture(scenario) { view ->
                    if (step > 0) {
                        val old = view.grid.rows
                        val upward = step % 3 != 0
                        val moves = if (upward) (0 until old.lastIndex).map { TerminalRowMove(it.toUShort(), (it + 1).toUShort()) }
                            else (1..old.lastIndex).map { TerminalRowMove(it.toUShort(), (it - 1).toUShort()) }
                        val index = if (upward) old.lastIndex else 0
                        val source = full.changedRows[step % old.size]
                        val newRow = source.copy(index = index.toUShort(), cells = source.cells.mapIndexed { column, cell ->
                            if (column < 8) cell.copy(text = "%08d".format(1000 + step)[column].toString()) else cell
                        })
                        assertTrue(view.grid.apply(full.copy(sequence = view.grid.sequence + 1u, full = false,
                            changedRows = listOf(newRow), rowMoves = moves)))
                        for (move in moves) assertSame(old[move.previous.toInt()], view.grid.rows[move.index.toInt()])
                    }
                }
                val expected = capture(scenario) { it.cacheRows = false; it.batchGlyphs = false }
                try {
                    assertTrue("Scrolled hardware raster differs from legacy at step $step", actual.sameAs(expected))
                } finally {
                    actual.recycle(); expected.recycle()
                }
                // Rewarm the cache, then move these nodes on the next update.
                capture(scenario) { it.cacheRows = true; it.batchGlyphs = true }.recycle()
            }
        }
    }

    @Test fun retainedRowsMatchIndependentLegacyViewAcrossCacheEvictions() {
        ActivityScenario.launch(TerminalProbeActivity::class.java).use { scenario ->
            await(scenario) { it.grid.hasGrid }
            lateinit var cached: TerminalView
            lateinit var legacy: TerminalView
            scenario.onActivity { activity ->
                cached = TerminalView(activity)
                legacy = TerminalView(activity).apply { cacheRows = false; batchGlyphs = false }
                val layout = android.widget.LinearLayout(activity).apply {
                    orientation = android.widget.LinearLayout.VERTICAL
                    val insets = activity.window.decorView.rootWindowInsets.getInsets(android.view.WindowInsets.Type.systemBars())
                    setPadding(insets.left, insets.top, insets.right, insets.bottom)
                    addView(cached, android.widget.LinearLayout.LayoutParams(-1, 0, 1f))
                    addView(legacy, android.widget.LinearLayout.LayoutParams(-1, 0, 1f))
                }
                activity.setContentView(layout)
            }
            instrumentation.waitForIdleSync()
            SystemClock.sleep(200)
            repeat(100) { step ->
                var draws = 0
                var legacyDraws = 0
                var top = Rect()
                var bottom = Rect()
                scenario.onActivity {
                    draws = cached.drawTimings.count
                    legacyDraws = legacy.drawTimings.count
                    val columns = cached.currentGridSize()!!.columns
                    val rows = minOf(cached.currentGridSize()!!.rows, legacy.currentGridSize()!!.rows)
                    val original = terminalStressFrame(columns, rows, (step + 1).toULong())
                    val frame = original.copy(cursor = null, styles = original.styles + original.styles[0].copy(
                        italic = true, faint = true, underline = Underline.CURLY, strikethrough = true, overline = true),
                        changedRows = original.changedRows.mapIndexed { index, row ->
                            val cells = row.cells.mapIndexed { column, cell ->
                                (if (column < 8) cell.copy(text = "%08d".format(step + index)[column].toString()) else cell)
                                    .copy(style = ((if (step < 3) index else step + index) % 2).toUInt())
                            }.toMutableList()
                            cells[10] = TerminalCell("界", CellWidth.WIDE, 1u)
                            cells[11] = TerminalCell("", CellWidth.SPACER_TAIL, 1u)
                            cells[14] = TerminalCell("😀", CellWidth.WIDE, 0u)
                            cells[15] = TerminalCell("", CellWidth.SPACER_TAIL, 0u)
                            cells[18] = TerminalCell("e\u0301", CellWidth.NARROW, 2u)
                            row.copy(cells = cells)
                        })
                    cached.grid.apply(frame); legacy.grid.apply(frame)
                    cached.invalidate(); legacy.invalidate()
                    fun bounds(view: TerminalView): Rect {
                        val xy = IntArray(2)
                        view.getLocationInWindow(xy)
                        return Rect(xy[0], xy[1], xy[0] + view.width, xy[1] + minOf(cached.height, legacy.height))
                    }
                    top = bounds(cached); bottom = bounds(legacy)
                }
                await(scenario) { cached.drawTimings.count > draws && legacy.drawTimings.count > legacyDraws }
                instrumentation.waitForIdleSync()
                SystemClock.sleep(40)
                val screen = pixels(scenario, Rect(0, 0, top.right, bottom.bottom))
                val actual = Bitmap.createBitmap(screen, top.left, top.top, top.width(), top.height())
                val expected = Bitmap.createBitmap(screen, bottom.left, bottom.top, bottom.width(), bottom.height())
                try {
                    if (!actual.sameAs(expected)) {
                        val dir = java.io.File(instrumentation.targetContext.filesDir, "terminal-review").apply { mkdirs() }
                        java.io.File(dir, "scroll-actual.png").outputStream().use { actual.compress(Bitmap.CompressFormat.PNG, 100, it) }
                        java.io.File(dir, "scroll-expected.png").outputStream().use { expected.compress(Bitmap.CompressFormat.PNG, 100, it) }
                        fail("Retained row raster differs at eviction/scroll step $step")
                    }
                } finally {
                    actual.recycle(); expected.recycle(); screen.recycle()
                }
            }
            scenario.onActivity {
                assertTrue("Workload must exercise retained rows", cached.rowCacheHits > 0)
                assertTrue("Workload must evict rows", cached.rowCacheMisses > cached.rowCacheLimit)
                assertTrue("Off-tree rows must restore discarded HWUI display lists", cached.rowDisplayListRestores > cached.rowCacheMisses)
            }
        }
    }

    @Test fun comparesCorrectLegacyBatchedAndRetainedScrollPerformance() {
        val reports = mutableListOf<String>()
        ActivityScenario.launch(TerminalProbeActivity::class.java).use { scenario ->
            await(scenario) { it.grid.hasGrid }
            fun update(native: Boolean) {
                var sequence = 0uL
                var metrics = 0
                lateinit var timings: FrameTimings
                scenario.onActivity { activity ->
                    sequence = activity.terminalView()!!.grid.sequence
                    timings = activity.renderTimings
                    metrics = timings.count
                    if (native) activity.nativeScrollStep() else activity.fullScreenUpdate()
                }
                await(scenario) { view -> timings.count > metrics && (!native || view.grid.sequence > sequence) }
            }
            for (native in listOf(false, true)) {
                if (native) {
                    var sequence = 0uL
                    scenario.onActivity { sequence = it.terminalView()!!.grid.sequence; it.startNativeScroll() }
                    await(scenario) { it.grid.sequence != sequence && it.grid.cursor == null }
                }
                for (mode in 0..2) {
                    scenario.onActivity { activity ->
                        val view = activity.terminalView()!!
                        view.cacheRows = mode >= 2
                        view.batchGlyphs = mode >= 1
                    }
                    repeat(10) { update(native) }
                    scenario.onActivity { it.resetTimings() }
                    repeat(60) { update(native) }
                    scenario.onActivity { activity ->
                        val view = activity.terminalView()!!
                        assertTrue(view.drawTimings.count >= 60)
                        reports += "${if (native) "Native scroll" else "Dense full update"}, mode=$mode\n${activity.timingReport()}"
                    }
                }
            }
        }
        val report = reports.joinToString("\n\n")
        val dir = java.io.File(instrumentation.targetContext.filesDir, "terminal-review").apply { mkdirs() }
        java.io.File(dir, "scroll-performance.txt").writeText(report)
        instrumentation.sendStatus(0, android.os.Bundle().apply { putString("scroll_performance", report) })
    }

    @Test fun nativeScrollPixelsMatchFullSnapshotAndLegacyAtEveryStep() {
        ActivityScenario.launch(TerminalProbeActivity::class.java).use { scenario ->
            await(scenario) { it.grid.hasGrid }
            var sequence = 0uL
            scenario.onActivity { sequence = it.terminalView()!!.grid.sequence; it.startNativeScroll() }
            await(scenario) { it.grid.sequence > sequence && it.grid.cursor == null }
            repeat(12) { step ->
                scenario.onActivity { sequence = it.terminalView()!!.grid.sequence; it.nativeScrollStep() }
                await(scenario) { it.grid.sequence > sequence }
                val actual = capture(scenario) {}
                lateinit var movedRows: List<ResolvedRow>
                scenario.onActivity {
                    val view = it.terminalView()!!
                    movedRows = view.grid.rows
                    sequence = view.grid.sequence
                    assertTrue(view.sessionCall { requestFullFrame() })
                }
                await(scenario) { it.grid.sequence > sequence }
                scenario.onActivity {
                    assertEquals("Moved rows differ from native full snapshot at step $step", movedRows, it.terminalView()!!.grid.rows)
                }
                val full = capture(scenario) {}
                val expected = capture(scenario) { it.cacheRows = false; it.batchGlyphs = false }
                try {
                    assertTrue("Native moved-row pixels differ from full snapshot at step $step", actual.sameAs(full))
                    assertTrue("Native scroll changed hardware pixels at step $step", actual.sameAs(expected))
                } finally {
                    actual.recycle(); full.recycle(); expected.recycle()
                }
                capture(scenario) { it.cacheRows = true; it.batchGlyphs = true }.recycle()
            }
        }
    }
}
