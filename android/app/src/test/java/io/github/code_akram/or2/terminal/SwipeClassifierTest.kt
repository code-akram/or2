package io.github.code_akram.or2.terminal

import io.github.code_akram.or2.ffi.NavDirection
import io.github.code_akram.or2.ffi.TargetNav
import org.junit.Assert.*
import org.junit.Test

class SwipeClassifierTest {
    /** Slop 10, swipe distance 100, pinch slop 20 (in pixels). */
    private fun classifier() = SwipeClassifier(slop = 10f, distance = 100f, pinchSlop = 20f)

    /** One finger from (200, 300) through [moves] (offsets); returns what fired and whether it was claimed. */
    private fun oneFinger(vararg moves: Pair<Float, Float>): Pair<List<Swipe>, Boolean> {
        val swipes = classifier()
        swipes.down(200f, 300f)
        val fired = moves.mapNotNull { (dx, dy) -> swipes.move(1, 200f + dx, 300f + dy, 0f) }
        return fired to swipes.up()
    }

    @Test
    fun aHorizontalSwipeFiresOncePastTheDistanceInTheFingersDirection() {
        assertEquals(listOf(Swipe.LEFT) to true, oneFinger(-20f to 2f, -60f to 5f, -100f to 8f, -180f to 10f))
        assertEquals(listOf(Swipe.RIGHT) to true, oneFinger(30f to 0f, 101f to -3f))
        // Short of the distance: claimed (no scroll, no tap) but nothing fires.
        assertEquals(emptyList<Swipe>() to true, oneFinger(30f to 0f, 99f to 0f))
        // Within the slop the touch is still undecided and stays the terminal's (a tap).
        assertEquals(emptyList<Swipe>() to false, oneFinger(3f to 4f, -5f to 2f))
    }

    @Test
    fun verticalAndDiagonalMovesAreLeftToTheTerminal() {
        // Vertical scrolling, then a long sideways move: never a swipe, never claimed.
        assertEquals(emptyList<Swipe>() to false, oneFinger(1f to 15f, 2f to 80f, -150f to 90f))
        // Diagonal: sideways must be at least twice the vertical when the slop is passed.
        assertEquals(emptyList<Swipe>() to false, oneFinger(-12f to -8f, -200f to -10f))
        assertEquals(listOf(Swipe.LEFT) to true, oneFinger(-12f to -6f, -200f to -10f))
    }

    @Test
    fun twoFingerSwipesMoveBetweenPanesAndSessionsFromTheFingersCentre() {
        fun twoFingers(dx: Float, dy: Float, spanChange: Float = 0f): Pair<List<Swipe>, Boolean> {
            val swipes = classifier()
            swipes.down(100f, 500f)
            swipes.move(1, 102f, 500f, 0f)
            // The second finger lands: the centre jumps, which is not a move.
            swipes.move(2, 200f, 500f, 200f)
            val fired = (1..4).mapNotNull { step ->
                swipes.move(2, 200f + dx * step / 4, 500f + dy * step / 4, 200f + spanChange * step / 4)
            }
            return fired to swipes.up()
        }
        assertEquals(listOf(Swipe.TWO_LEFT) to true, twoFingers(-120f, 10f))
        assertEquals(listOf(Swipe.TWO_RIGHT) to true, twoFingers(120f, -10f))
        assertEquals(listOf(Swipe.TWO_UP) to true, twoFingers(5f, -150f))
        assertEquals(listOf(Swipe.TWO_DOWN) to true, twoFingers(-5f, 150f))
        // Not far enough, or neither axis dominant: nothing, but the touch is still the swipe's.
        assertEquals(emptyList<Swipe>() to true, twoFingers(-90f, 0f))
        assertEquals(emptyList<Swipe>() to true, twoFingers(-120f, -100f))
        // Pinch has priority: the span changing past the pinch slop is never also a swipe.
        assertEquals(emptyList<Swipe>() to true, twoFingers(-200f, 0f, spanChange = 100f))
        assertEquals(emptyList<Swipe>() to true, twoFingers(-200f, 0f, spanChange = -100f))
        assertEquals(listOf(Swipe.TWO_LEFT) to true, twoFingers(-200f, 0f, spanChange = 15f))
    }

    @Test
    fun aSecondFingerTurnsAScrollIntoATwoFingerSwipeButNotAfterASwipeFired() {
        val swipes = classifier()
        swipes.down(100f, 500f)
        assertNull(swipes.move(1, 100f, 530f, 0f)) // Scrolling: the terminal's.
        assertFalse(swipes.claimed)
        swipes.move(2, 150f, 530f, 100f)
        assertTrue(swipes.claimed)
        assertEquals(Swipe.TWO_DOWN, swipes.move(2, 150f, 650f, 100f))
        assertNull(swipes.move(2, 150f, 800f, 100f)) // Once per touch.
        assertTrue(swipes.up())

        swipes.down(100f, 500f)
        assertEquals(Swipe.LEFT, swipes.move(1, -10f, 500f, 0f))
        swipes.move(2, -10f, 500f, 100f)
        assertNull(swipes.move(2, -10f, 300f, 100f))
        assertTrue(swipes.up())
    }

    @Test
    fun aLiftedFingerOrAThirdOneEndsClassificationButTheTouchStaysClaimed() {
        val swipes = classifier()
        swipes.down(100f, 500f)
        swipes.move(2, 150f, 500f, 100f)
        swipes.move(1, 100f, 500f, 0f) // One finger lifted.
        assertNull(swipes.move(1, -200f, 500f, 0f))
        assertTrue(swipes.up())

        swipes.down(100f, 500f)
        swipes.move(2, 150f, 500f, 100f)
        swipes.move(3, 150f, 500f, 100f)
        assertNull(swipes.move(3, -200f, 500f, 100f))
        assertTrue(swipes.up())
    }

    @Test
    fun aPinchOrASelectionCancelsTheTouch() {
        val swipes = classifier()
        swipes.down(100f, 500f)
        swipes.move(2, 150f, 500f, 100f)
        assertTrue(swipes.claimed)
        swipes.cancel() // The pinch detector started.
        assertFalse(swipes.claimed)
        assertNull(swipes.move(2, -100f, 500f, 100f))
        assertFalse(swipes.up())

        // A long press began a selection before the finger moved: its drag is never a swipe.
        swipes.down(100f, 500f)
        swipes.cancel()
        assertNull(swipes.move(1, -200f, 500f, 0f))
        assertFalse(swipes.up())
        // Moves without a touch are ignored.
        assertNull(swipes.move(1, -200f, 500f, 0f))
    }

    @Test
    fun swipesMapToTheirMoves() {
        assertEquals(TargetNav.NextWindow, swipeNav(Swipe.LEFT))
        assertEquals(TargetNav.PreviousWindow, swipeNav(Swipe.RIGHT))
        assertEquals(TargetNav.Pane(NavDirection.LEFT), swipeNav(Swipe.TWO_LEFT))
        assertEquals(TargetNav.Pane(NavDirection.RIGHT), swipeNav(Swipe.TWO_RIGHT))
        assertEquals(TargetNav.NextSession, swipeNav(Swipe.TWO_UP))
        assertEquals(TargetNav.PreviousSession, swipeNav(Swipe.TWO_DOWN))
    }
}
