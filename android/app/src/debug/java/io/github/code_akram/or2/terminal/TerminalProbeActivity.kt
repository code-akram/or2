package io.github.code_akram.or2.terminal

import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.view.FrameMetrics
import android.view.View
import android.view.ViewGroup
import android.view.Window
import android.view.inputmethod.InputMethodManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import io.github.code_akram.or2.ffi.LinkHealth
import io.github.code_akram.or2.ui.Or2Theme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import io.github.code_akram.or2.ffi.CursorShape
import io.github.code_akram.or2.ffi.Session
import io.github.code_akram.or2.ffi.SessionListener
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalFrame
import io.github.code_akram.or2.ffi.contractProbeSession
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.receiveAsFlow

/** Debug-only, local native contract fixture. Never connects to a host. */
class TerminalProbeActivity : ComponentActivity() {
    private lateinit var session: Session
    private val state = MutableStateFlow<SessionState>(SessionState.Connecting)
    private val frames = Channel<Unit>(Channel.CONFLATED)
    private val frameFlow = frames.receiveAsFlow()
    val renderTimings = FrameTimings()
    val gpuTimings = FrameTimings()
    val windowDrawTimings = FrameTimings()
    val syncTimings = FrameTimings()
    val commandIssueTimings = FrameTimings()
    var droppedFrameMetrics = 0
        private set
    private var stats by mutableStateOf("")
    private var shapeIndex = 0
    private var stressSequence = 0uL
    private var nativeScrolling = false
    private val metricsListener = Window.OnFrameMetricsAvailableListener { _, metrics, dropped ->
        // Main-thread delivery keeps the rolling samples and test reads single-threaded.
        renderTimings.record(metrics.getMetric(FrameMetrics.TOTAL_DURATION))
        // -1 means this metric was unavailable; do not turn it into a negative duration.
        fun record(metric: Int, timings: FrameTimings) {
            val duration = metrics.getMetric(metric)
            if (duration >= 0) timings.record(duration)
        }
        record(FrameMetrics.GPU_DURATION, gpuTimings)
        record(FrameMetrics.DRAW_DURATION, windowDrawTimings)
        record(FrameMetrics.SYNC_DURATION, syncTimings)
        record(FrameMetrics.COMMAND_ISSUE_DURATION, commandIssueTimings)
        droppedFrameMetrics += dropped
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.addOnFrameMetricsAvailableListener(metricsListener, Handler(Looper.getMainLooper()))
        session = contractProbeSession(
            40u, 12u,
            object : SessionListener {
                override fun onStateChanged(state: SessionState) {
                    this@TerminalProbeActivity.state.value = state
                }
                override fun onFrameReady() { frames.trySend(Unit) }
                override fun onLinkHealth(health: LinkHealth) = Unit
                override fun onClipboardWrite(text: String) = Unit
                override fun onServerPid(pid: UInt) = Unit
            },
        )
        setContent {
            Or2Theme {
                Column(Modifier.windowInsetsPadding(WindowInsets.safeDrawing)) {
                    Row(Modifier.horizontalScroll(rememberScrollState())) {
                        TextButton(onClick = { requestProbeFrame() }) { Text("Probe") }
                        TextButton(onClick = {
                            val shape = CursorShape.entries[shapeIndex++ % CursorShape.entries.size]
                            terminalView()?.let { view ->
                                display(terminalVisualFrame(view.grid.columns.toUShort(), view.grid.rows.size.toUShort(), shape))
                            }
                        }) { Text("Styles / cursor") }
                        TextButton(onClick = { fullScreenUpdate() }) { Text("Full update") }
                        TextButton(onClick = { if (nativeScrolling) nativeScrollStep() else startNativeScroll() }) { Text("Native scroll") }
                        TextButton(onClick = { terminalView()?.input?.compose("e\u0301界😀") }) { Text("Compose") }
                        TextButton(onClick = {
                            terminalView()?.let { view ->
                                if (view.grid.rows.size > 1 && view.grid.columns > 6) {
                                    view.beginSelection(CellPosition(1, 1))
                                    view.selection?.end = CellPosition(5, 1)
                                    view.invalidate()
                                }
                            }
                        }) { Text("Select") }
                        TextButton(onClick = {
                            terminalView()?.let { view ->
                                view.showTimings = !view.showTimings
                                stats = timingReport()
                                view.invalidate()
                            }
                        }) { Text("Stats") }
                        TextButton(onClick = {
                            terminalView()?.let { view ->
                                view.directLatinInput = !view.directLatinInput
                                view.input.discardComposition()
                                getSystemService(InputMethodManager::class.java).restartInput(view)
                                stats = if (view.directLatinInput) "DEBUG visible-password IME: test Latin latency and CJK" else "Default composing text IME"
                            }
                        }) { Text("IME comparison") }
                    }
                    if (stats.isNotEmpty()) Text(stats)
                    TerminalScreen(session, state, frameFlow, Modifier.weight(1f))
                }
            }
        }
    }

    /** Mirrors the session screen's Compose siblings, with visible margins for clip checks. */
    fun showBoundsFixture() {
        setContent {
            Or2Theme {
                Column(Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.safeDrawing)
                    .background(Color(0xff336699))) {
                    Text("Compose session title above terminal", color = Color.White,
                        modifier = Modifier.fillMaxWidth().height(64.dp))
                    TerminalScreen(session, state, frameFlow, Modifier.weight(1f).padding(horizontal = 16.dp))
                }
            }
        }
    }

    fun terminalView(): TerminalView? {
        fun find(view: View): TerminalView? {
            if (view is TerminalView) return view
            if (view is ViewGroup) for (index in 0 until view.childCount) find(view.getChildAt(index))?.let { return it }
            return null
        }
        return find(window.decorView)
    }

    fun display(frame: TerminalFrame) {
        terminalView()?.let { view ->
            view.clearSelection()
            view.input.discardComposition()
            val start = System.nanoTime()
            val changed = view.grid.apply(frame)
            check(!view.grid.needsFullFrame)
            val elapsed = System.nanoTime() - start
            view.applyTimings.record(elapsed)
            view.mergeTimings.record(elapsed)
            if (changed) view.invalidate()
        }
    }

    fun fullScreenUpdate() {
        terminalView()?.let { view ->
            display(terminalStressFrame(view.grid.columns.toUShort(), view.grid.rows.size.toUShort(), ++stressSequence))
        }
    }

    fun requestProbeFrame() {
        terminalView()?.let { view ->
            if (view.selection != null) view.clearSelection()
            if (view.input.composing.isNotEmpty()) view.input.discardComposition()
            view.sessionCall {
                if (nativeScrolling) sendText("\u001bor2:scroll:stop") else requestFullFrame()
            }
            nativeScrolling = false
        }
    }

    fun startNativeScroll() {
        terminalView()?.let { view ->
            view.clearSelection()
            view.input.discardComposition()
            nativeScrolling = true
            view.sessionCall { sendText("\u001bor2:scroll:start") }
        }
    }

    fun nativeScrollStep() {
        terminalView()?.sessionCall { sendText("\u001bor2:scroll:step") }
    }

    fun resetTimings() {
        renderTimings.clear()
        gpuTimings.clear()
        windowDrawTimings.clear()
        syncTimings.clear()
        commandIssueTimings.clear()
        droppedFrameMetrics = 0
        terminalView()?.apply {
            applyTimings.clear()
            takeTimings.clear()
            mergeTimings.clear()
            ingressTimings.clear()
            drawTimings.clear()
            resetRowCacheCounters()
        }
    }

    fun timingReport(): String {
        fun summary(label: String, timings: FrameTimings) =
            "$label n=${timings.count} p50=%.3f p95=%.3f p99=%.3f ms".format(
                timings.percentile(50), timings.percentile(95), timings.percentile(99))
        val view = terminalView() ?: return "No terminal view"
        val lookups = view.rowCacheHits + view.rowCacheMisses
        val hitRate = if (lookups == 0L) 0.0 else 100.0 * view.rowCacheHits / lookups
        return listOf(summary("apply", view.applyTimings), summary("native take/decode", view.takeTimings),
            summary("grid merge", view.mergeTimings), summary("read-request-to-apply latency", view.ingressTimings),
            summary("CPU record", view.drawTimings),
            summary("Window TOTAL_DURATION", renderTimings), summary("Window GPU_DURATION", gpuTimings),
            summary("Window DRAW_DURATION", windowDrawTimings), summary("Window SYNC_DURATION", syncTimings),
            summary("Window COMMAND_ISSUE_DURATION", commandIssueTimings),
            "row cache hits=${view.rowCacheHits} misses=${view.rowCacheMisses} hit=%.1f%% retained=${view.rowCacheSize}/${view.rowCacheLimit}".format(hitRate),
            "row display lists restored=${view.rowDisplayListRestores}",
            "metrics callbacks dropped=$droppedFrameMetrics").joinToString("\n")
    }

    override fun onDestroy() {
        window.removeOnFrameMetricsAvailableListener(metricsListener)
        session.disconnect()
        session.close()
        frames.close()
        super.onDestroy()
    }
}
