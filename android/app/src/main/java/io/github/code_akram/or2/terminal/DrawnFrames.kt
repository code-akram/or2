package io.github.code_akram.or2.terminal

/**
 * Tells the timing markers that a frame the view applied has really been drawn. A frame is
 * [applied] when the grid took it (and the view was invalidated), and [drawn] is asked at the end
 * of `onDraw`: it answers true exactly once per applied frame, so the report follows the draw and
 * never precedes it, and it works for every later activation of a retained view (a terminal
 * shown again after the app returned from the background), not only the view's first frame. A draw
 * with no new frame (the cursor blinking, a selection) reports nothing.
 */
internal class AppliedFrames {
    private var pending = false

    /** The grid applied a frame and the view asked to be redrawn. */
    fun applied() {
        pending = true
    }

    /**
     * The view is shown (again): its next draw is reported even when no new frame arrives, because
     * what a waiting path needs is the terminal on screen, and a retained view's content may not
     * have changed while it was away.
     */
    fun shown() {
        pending = true
    }

    /** The view finished drawing: whether that draw showed a frame that was applied since the last one. */
    fun drawn(): Boolean = pending.also { pending = false }
}
