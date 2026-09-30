package io.github.code_akram.or2.terminal

import android.graphics.Bitmap
import android.graphics.Rect
import android.os.Bundle
import android.os.SystemClock
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.github.code_akram.or2.ffi.CursorShape
import java.io.File
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

/** Captures only synthetic terminal content, under the target app's private files directory. */
@RunWith(AndroidJUnit4::class)
class TerminalVisualDeviceTest {
    private val instrumentation get() = InstrumentationRegistry.getInstrumentation()

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

    private fun capture(scenario: ActivityScenario<TerminalProbeActivity>, name: String) {
        // Settle Compose key state and hardware render submission before asking SurfaceFlinger.
        instrumentation.waitForIdleSync()
        SystemClock.sleep(100)
        val bounds = Rect()
        scenario.onActivity { activity ->
            activity.window.decorView.getWindowVisibleDisplayFrame(bounds)
            val view = activity.terminalView()!!
            val position = IntArray(2)
            view.getLocationOnScreen(position)
            bounds.top = position[1] // Exclude system status and debug toolbar; retain terminal + keys.
        }
        val screenshot = instrumentation.uiAutomation.takeScreenshot()
        assertNotNull("Hardware screenshot unavailable", screenshot)
        val cropped = Bitmap.createBitmap(screenshot!!, bounds.left, bounds.top,
            bounds.width().coerceAtMost(screenshot.width - bounds.left), bounds.height().coerceAtMost(screenshot.height - bounds.top))
        val directory = File(instrumentation.targetContext.filesDir, "terminal-review").apply { mkdirs() }
        File(directory, "$name.png").outputStream().use { assertTrue(cropped.compress(Bitmap.CompressFormat.PNG, 100, it)) }
        cropped.recycle()
        screenshot.recycle()
    }

    @Test fun capturesProbeStylesWideCursorsCompositionKeysAndSelection() {
        ActivityScenario.launch(TerminalProbeActivity::class.java).use { scenario ->
            await(scenario) { _, view -> view.grid.hasGrid && view.grid.rows.size > 12 }
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
