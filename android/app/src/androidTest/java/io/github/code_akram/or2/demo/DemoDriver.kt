package io.github.code_akram.or2.demo

import android.os.SystemClock
import android.view.InputDevice
import android.view.MotionEvent
import android.view.View
import android.view.ViewGroup
import android.view.inspector.WindowInspector
import androidx.compose.ui.node.RootForTest
import androidx.compose.ui.semantics.SemanticsNode
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONArray
import org.json.JSONObject

/**
 * Drives the real app the way a finger does: nodes are found in the live Compose semantics of every window (sheets are
 * windows of their own), and taps and drags go through the system's input path, so ripples, sheets and the keyboard
 * behave as they do by hand. No Compose test rule: its test clock would drive the app's animations instead of the
 * display, and the recording would show them jump. Every touch is logged against [t0] for the GIF's touch marks.
 */
internal class DemoDriver {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val automation = instrumentation.uiAutomation

    /** Uptime of the first frame without the sync marker: time zero of the timeline (and of the trimmed video). */
    var t0 = 0L
    private val events = JSONArray()

    fun now() = SystemClock.uptimeMillis() - t0

    fun scene(index: Int, caption: String) {
        events.put(JSONObject().put("t", now()).put("kind", "scene").put("index", index).put("caption", caption))
    }

    /** What the GIF renderer reads: the screen (pixels; the system bars it crops away), the frames' width, the events. */
    fun timeline(screen: JSONObject, frameWidth: Int): JSONObject =
        JSONObject().put("screen", screen).put("frameWidth", frameWidth).put("events", events)

    /** The centre of the first node matching [match] on screen, in screen pixels; the topmost window wins. */
    private fun locate(match: (SemanticsNode) -> Boolean): Pair<Float, Float>? {
        var found: Pair<Float, Float>? = null
        instrumentation.runOnMainSync {
            for (root in WindowInspector.getGlobalWindowViews().reversed()) {
                val compose = composeRoots(root)
                for (owner in compose) {
                    val node = search(owner.semanticsOwner.unmergedRootSemanticsNode, match) ?: continue
                    val position = node.positionOnScreen
                    found = (position.x + node.size.width / 2f) to (position.y + node.size.height / 2f)
                    return@runOnMainSync
                }
            }
        }
        return found
    }

    /** The first node matching [match], topmost window first. Call on the main thread. */
    fun node(match: (SemanticsNode) -> Boolean): SemanticsNode? {
        for (root in WindowInspector.getGlobalWindowViews().reversed()) {
            for (owner in composeRoots(root)) search(owner.semanticsOwner.unmergedRootSemanticsNode, match)?.let { return it }
        }
        return null
    }

    private fun composeRoots(view: View): List<RootForTest> = when {
        view is RootForTest -> listOf(view)
        view is ViewGroup -> (0 until view.childCount).flatMap { composeRoots(view.getChildAt(it)) }
        else -> emptyList()
    }

    private fun search(node: SemanticsNode, match: (SemanticsNode) -> Boolean): SemanticsNode? {
        if (match(node)) return node
        for (child in node.children) search(child, match)?.let { return it }
        return null
    }

    fun tag(tag: String): (SemanticsNode) -> Boolean = { it.config.getOrNull(SemanticsProperties.TestTag) == tag }

    fun text(text: String): (SemanticsNode) -> Boolean =
        { node -> node.config.getOrNull(SemanticsProperties.Text)?.any { it.text == text } == true }

    fun await(what: String, timeoutMs: Long = 8_000, match: (SemanticsNode) -> Boolean): Pair<Float, Float> {
        val deadline = SystemClock.uptimeMillis() + timeoutMs
        while (true) {
            locate(match)?.let { return it }
            check(SystemClock.uptimeMillis() < deadline) { "Timed out waiting for $what" }
            SystemClock.sleep(50)
        }
    }

    fun exists(match: (SemanticsNode) -> Boolean) = locate(match) != null

    /** A finger's tap on [what]: down, a short press, up. */
    fun tap(what: String, match: (SemanticsNode) -> Boolean) {
        val (x, y) = await(what, match = match)
        events.put(JSONObject().put("t", now()).put("kind", "tap").put("x", x.toDouble()).put("y", y.toDouble()))
        val down = SystemClock.uptimeMillis()
        inject(MotionEvent.ACTION_DOWN, down, down, x, y)
        SystemClock.sleep(95)
        inject(MotionEvent.ACTION_UP, down, SystemClock.uptimeMillis(), x, y)
    }

    /** A finger's drag from ([x], [y]) by [dy] pixels over [durationMs], eased, ending still. */
    fun drag(x: Float, y: Float, dy: Float, durationMs: Long) {
        events.put(JSONObject().put("t", now()).put("kind", "drag").put("x", x.toDouble()).put("y", y.toDouble())
            .put("dy", dy.toDouble()).put("ms", durationMs))
        val down = SystemClock.uptimeMillis()
        inject(MotionEvent.ACTION_DOWN, down, down, x, y)
        val steps = (durationMs / 8).toInt().coerceAtLeast(2)
        for (step in 1..steps) {
            val progress = step / steps.toFloat()
            val eased = 1 - (1 - progress) * (1 - progress)
            SystemClock.sleep(durationMs / steps)
            inject(MotionEvent.ACTION_MOVE, down, SystemClock.uptimeMillis(), x, y + dy * eased)
        }
        inject(MotionEvent.ACTION_UP, down, SystemClock.uptimeMillis(), x, y + dy)
    }

    private fun inject(action: Int, down: Long, at: Long, x: Float, y: Float) {
        val event = MotionEvent.obtain(down, at, action, x, y, 0).apply { source = InputDevice.SOURCE_TOUCHSCREEN }
        check(automation.injectInputEvent(event, true)) { "The system refused a touch" }
        event.recycle()
    }

    /**
     * Films the screen into [dir] as numbered JPEGs [width] pixels wide, with `frames.txt` listing each frame's time on
     * the timeline. Screenshots, not `screenrecord`: on Android 16 builds that run it in its own SELinux domain it can
     * neither write a file nor start. Frames are taken at up to [fps] and encoded on other threads; a frame is dropped
     * rather than queued when the encoders fall behind, so the app is never starved.
     */
    inner class Recorder(private val dir: java.io.File, private val width: Int, private val fps: Int = 30) {
        private val running = java.util.concurrent.atomic.AtomicBoolean(false)
        private val pending = java.util.concurrent.atomic.AtomicInteger()
        private val encoders = java.util.concurrent.Executors.newFixedThreadPool(3)
        private val frames = java.util.concurrent.ConcurrentSkipListMap<Int, Long>()
        private var dropped = 0
        private var thread: Thread? = null

        fun start() {
            dir.deleteRecursively()
            dir.mkdirs()
            running.set(true)
            thread = Thread {
                var index = 0
                val period = 1_000L / fps
                while (running.get()) {
                    val started = SystemClock.uptimeMillis()
                    val shot = automation.takeScreenshot()
                    val at = (started + SystemClock.uptimeMillis()) / 2 - t0
                    if (shot != null) {
                        if (pending.get() >= 6) {
                            dropped++
                            shot.recycle()
                        } else {
                            val frame = index++
                            pending.incrementAndGet()
                            encoders.execute { encode(shot, frame, at) }
                        }
                    }
                    val left = period - (SystemClock.uptimeMillis() - started)
                    if (left > 0) SystemClock.sleep(left)
                }
            }.apply { start() }
        }

        private fun encode(shot: android.graphics.Bitmap, frame: Int, at: Long) {
            try {
                val soft = if (shot.config == android.graphics.Bitmap.Config.HARDWARE) shot.copy(android.graphics.Bitmap.Config.ARGB_8888, false) else shot
                val height = Math.round(soft.height * width / soft.width.toFloat())
                val scaled = android.graphics.Bitmap.createScaledBitmap(soft, width, height, true)
                java.io.File(dir, "%05d.jpg".format(frame)).outputStream().buffered().use {
                    scaled.compress(android.graphics.Bitmap.CompressFormat.JPEG, 93, it)
                }
                frames[frame] = at
                if (scaled !== soft) scaled.recycle()
                if (soft !== shot) soft.recycle()
                shot.recycle()
            } finally {
                pending.decrementAndGet()
            }
        }

        /** Stops filming, waits for the encoders, and writes `frames.txt` (`index time_ms`, one per line). */
        fun stop(): String {
            running.set(false)
            thread?.join()
            encoders.shutdown()
            encoders.awaitTermination(30, java.util.concurrent.TimeUnit.SECONDS)
            java.io.File(dir, "frames.txt").writeText(frames.entries.joinToString("") { "%05d %d\n".format(it.key, it.value) })
            return "${frames.size} frames, $dropped dropped"
        }
    }
}
