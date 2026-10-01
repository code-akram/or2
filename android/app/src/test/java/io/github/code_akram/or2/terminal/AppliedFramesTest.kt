package io.github.code_akram.or2.terminal

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** When the view tells the timing markers a frame was drawn. */
class AppliedFramesTest {
    @Test
    fun nothingIsReportedBeforeTheDrawAndAFrameIsReportedOncePerApply() {
        val frames = AppliedFrames()
        assertFalse(frames.drawn()) // A draw before any frame was applied (the cursor, a resize).
        frames.applied()
        // The apply itself reports nothing: only the draw that follows does, once.
        assertTrue(frames.drawn())
        assertFalse(frames.drawn()) // The next draw has no new frame.
    }

    @Test
    fun everyLaterFrameOfARetainedViewIsReportedNotOnlyTheFirst() {
        val frames = AppliedFrames()
        repeat(3) {
            frames.applied()
            assertTrue(frames.drawn())
            assertFalse(frames.drawn())
        }
    }

    @Test
    fun aRetainedViewShownAgainReportsItsNextDrawEvenWithoutANewFrame() {
        val frames = AppliedFrames()
        frames.applied()
        assertTrue(frames.drawn())
        // Backgrounded and returned: the terminal's content did not change, but it is on screen again.
        frames.shown()
        assertTrue(frames.drawn())
        assertFalse(frames.drawn())
    }

    @Test
    fun framesAppliedBeforeOneDrawAreOneReport() {
        val frames = AppliedFrames()
        frames.applied()
        frames.applied()
        assertTrue(frames.drawn())
        assertFalse(frames.drawn())
    }
}
