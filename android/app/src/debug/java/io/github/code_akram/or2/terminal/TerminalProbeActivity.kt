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
import io.github.code_akram.or2.ffi.ConnectRequest
import io.github.code_akram.or2.ffi.CursorShape
import io.github.code_akram.or2.ffi.Session
import io.github.code_akram.or2.ffi.SessionListener
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalFrame
import io.github.code_akram.or2.ffi.contractProbeSession
import io.github.code_akram.or2.ffi.generateEd25519Key
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
    var droppedFrameMetrics = 0
        private set
    private var stats by mutableStateOf("")
    private var shapeIndex = 0
    private var stressSequence = 0uL
    private val metricsListener = Window.OnFrameMetricsAvailableListener { _, metrics, dropped ->
        // Main-thread delivery keeps the rolling samples and test reads single-threaded.
        renderTimings.record(metrics.getMetric(FrameMetrics.TOTAL_DURATION))
        droppedFrameMetrics += dropped
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.addOnFrameMetricsAvailableListener(metricsListener, Handler(Looper.getMainLooper()))
        val key = generateEd25519Key("terminal probe")
        try {
            session = contractProbeSession(
                ConnectRequest("probe.invalid", 22u, "probe", key.privateKey, emptyList(), 40u, 12u),
                object : SessionListener {
                    override fun onStateChanged(state: SessionState) {
                        this@TerminalProbeActivity.state.value = state
                        if (state is SessionState.AwaitingHostKeyDecision) {
                            // Posting defers until the synchronous factory has assigned the handle.
                            runOnUiThread { session.approveHostKey(state.presented.fingerprint) }
                        }
                    }
                    override fun onFrameReady() { frames.trySend(Unit) }
                    override fun onLinkHealth(health: LinkHealth) = Unit
                    override fun onClipboardWrite(text: String) = Unit
                },
            )
        } finally {
            key.privateKey.fill(0)
        }
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
            check(view.grid.apply(frame))
            view.applyTimings.record(System.nanoTime() - start)
            view.invalidate()
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
            view.sessionCall { requestFullFrame() }
        }
    }

    fun resetTimings() {
        renderTimings.clear()
        droppedFrameMetrics = 0
        terminalView()?.apply {
            applyTimings.clear()
            drawTimings.clear()
        }
    }

    fun timingReport(): String {
        fun summary(label: String, timings: FrameTimings) =
            "$label n=${timings.count} p50=%.3f p95=%.3f p99=%.3f ms".format(
                timings.percentile(50), timings.percentile(95), timings.percentile(99))
        val view = terminalView() ?: return "No terminal view"
        return listOf(summary("apply", view.applyTimings), summary("CPU record", view.drawTimings),
            summary("Window TOTAL_DURATION", renderTimings), "metrics callbacks dropped=$droppedFrameMetrics").joinToString("\n")
    }

    override fun onDestroy() {
        window.removeOnFrameMetricsAvailableListener(metricsListener)
        session.disconnect()
        session.close()
        frames.close()
        super.onDestroy()
    }
}
