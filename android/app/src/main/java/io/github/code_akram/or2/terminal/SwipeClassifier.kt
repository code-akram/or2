package io.github.code_akram.or2.terminal

import io.github.code_akram.or2.ffi.NavDirection
import io.github.code_akram.or2.ffi.TargetNav
import kotlin.math.abs

/** A navigation swipe on the terminal: the direction the finger(s) moved. */
enum class Swipe { LEFT, RIGHT, TWO_LEFT, TWO_RIGHT, TWO_UP, TWO_DOWN }

/**
 * What a swipe asks of the tmux or herdr target: one finger left is the next window (tab), right the
 * previous; two fingers left or right the pane that way; two fingers up the next session (workspace),
 * down the previous.
 */
fun swipeNav(swipe: Swipe): TargetNav = when (swipe) {
    Swipe.LEFT -> TargetNav.NextWindow
    Swipe.RIGHT -> TargetNav.PreviousWindow
    Swipe.TWO_LEFT -> TargetNav.Pane(NavDirection.LEFT)
    Swipe.TWO_RIGHT -> TargetNav.Pane(NavDirection.RIGHT)
    Swipe.TWO_UP -> TargetNav.NextSession
    Swipe.TWO_DOWN -> TargetNav.PreviousSession
}

/**
 * Tells navigation swipes from the terminal's other touches, one touch at a time (all fingers down to
 * all fingers up), from pointer positions alone so it is plain JVM code.
 *
 * - **One finger** decides once it has moved [slop] (in any direction): mostly sideways (horizontal at least twice the
 *   vertical) makes it a horizontal swipe, anything else leaves the touch to the terminal (vertical
 *   scrolling, taps, long-press selection are untouched). A horizontal swipe fires once it has gone
 *   [distance] sideways.
 * - **Two fingers** (a second finger down at any time before a swipe fired, also after the first
 *   one started to scroll) make it a two-finger
 *   swipe, measured from the fingers' centre once both are down. It fires once the centre has gone
 *   [distance] along one axis, at least twice as far as along the other. **Pinch has priority:** a
 *   change of the fingers' span past [pinchSlop] (the platform's own pinch slop) means a pinch, and
 *   no swipe fires for the rest of the touch; the view also [cancel]s when its pinch detector starts.
 * - A third finger ends classification. At most one swipe fires per touch.
 *
 * Once a touch is a swipe ([claimed]) the classifier owns it to the end: the view cancels its other
 * detectors and consumes the remaining events. The view does not start one during a selection.
 */
class SwipeClassifier(private val slop: Float, private val distance: Float, private val pinchSlop: Float) {
    /** PASSED: one finger moved some other way; the touch is the terminal's unless a second finger lands. */
    private enum class Mode { IDLE, UNDECIDED, HORIZONTAL, PASSED, TWO, DONE }

    private var mode = Mode.IDLE
    private var pointers = 0
    private var startX = 0f
    private var startY = 0f
    private var startSpan = 0f

    /** The touch is a navigation swipe: its events belong to the classifier until all fingers are up. */
    var claimed = false
        private set

    /** The first finger went down at ([x], [y]). */
    fun down(x: Float, y: Float) {
        mode = Mode.UNDECIDED
        claimed = false
        pointers = 1
        startX = x
        startY = y
    }

    /**
     * The touch now has [count] fingers down, centred at ([x], [y]) with [span] between the first two
     * (0 for one finger). Call it for every move and every finger going down or up. Returns the swipe
     * this event completes, at most once per touch.
     */
    fun move(count: Int, x: Float, y: Float, span: Float): Swipe? {
        val previous = pointers
        pointers = count
        if (mode == Mode.IDLE || mode == Mode.DONE) return null
        if (count != previous) {
            when {
                count == 2 && previous == 1 -> {
                    mode = Mode.TWO
                    claimed = true
                    startX = x
                    startY = y
                    startSpan = span
                }
                // A finger lifted from a two-finger swipe, or a third one landed: nothing more here.
                else -> finish()
            }
            return null
        }
        val dx = x - startX
        val dy = y - startY
        return when (mode) {
            Mode.UNDECIDED -> {
                // Euclidean, like the platform's own scroll slop, so this decides no later than it.
                if (dx * dx + dy * dy < slop * slop) return null
                if (abs(dx) >= 2 * abs(dy)) {
                    mode = Mode.HORIZONTAL
                    claimed = true
                    horizontal(dx)
                } else {
                    mode = Mode.PASSED // Vertical or diagonal: the terminal's own gesture.
                    null
                }
            }
            Mode.HORIZONTAL -> horizontal(dx)
            Mode.PASSED -> null
            Mode.TWO -> {
                if (abs(span - startSpan) > pinchSlop) {
                    finish() // A pinch: never also a swipe.
                    return null
                }
                val swipe = when {
                    abs(dx) >= distance && abs(dx) >= 2 * abs(dy) -> if (dx < 0) Swipe.TWO_LEFT else Swipe.TWO_RIGHT
                    abs(dy) >= distance && abs(dy) >= 2 * abs(dx) -> if (dy < 0) Swipe.TWO_UP else Swipe.TWO_DOWN
                    else -> null
                }
                swipe?.also { finish() }
            }
            else -> null
        }
    }

    private fun horizontal(dx: Float): Swipe? {
        if (abs(dx) < distance) return null
        finish()
        return if (dx < 0) Swipe.LEFT else Swipe.RIGHT
    }

    /** Nothing more fires in this touch; a claimed touch stays claimed until [up]. */
    private fun finish() {
        mode = Mode.DONE
    }

    /** A pinch or a selection started: this touch is not a swipe, and was never claimed. */
    fun cancel() {
        mode = Mode.DONE
        claimed = false
    }

    /** All fingers are up (or the touch was cancelled). Returns whether the touch was a swipe's. */
    fun up(): Boolean {
        val wasClaimed = claimed
        mode = Mode.IDLE
        claimed = false
        pointers = 0
        return wasClaimed
    }
}
