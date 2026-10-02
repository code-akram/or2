package io.github.code_akram.or2.ui

import androidx.compose.ui.unit.dp
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** The top bar's scroll edge (docs/ui.md, "Top bar"): shown exactly while content sits under the bar. */
class ScrollEdgeTest {
    @Test
    fun aScrollingColumnShowsTheEdgeOnceItLeavesTheTop() {
        assertFalse(isScrolledUnder(0))
        assertTrue(isScrolledUnder(1))
        assertTrue(isScrolledUnder(2_000))
    }

    @Test
    fun aLazyListShowsTheEdgeOnceItsFirstItemMoves() {
        // At the very top: no edge.
        assertFalse(isScrolledUnder(firstVisibleItemIndex = 0, firstVisibleItemScrollOffset = 0))
        // The first item partly under the bar, or gone: the edge.
        assertTrue(isScrolledUnder(firstVisibleItemIndex = 0, firstVisibleItemScrollOffset = 1))
        assertTrue(isScrolledUnder(firstVisibleItemIndex = 1, firstVisibleItemScrollOffset = 0))
        assertTrue(isScrolledUnder(firstVisibleItemIndex = 7, firstVisibleItemScrollOffset = 40))
    }

    @Test
    fun everyTopBarIsOneHeightAndSheetsStopBelowTheStatusBar() {
        assertEquals(48.dp, Or2Dimens.TopBar)
        // The bar's icon buttons sit on the screen edges: their glyphs land on the gutter.
        assertEquals(Or2Dimens.Gutter, (Or2Dimens.IconButton - Or2Dimens.Icon) / 2)
        assertEquals(12.dp, Or2Dimens.SheetTopGap)
    }
}
