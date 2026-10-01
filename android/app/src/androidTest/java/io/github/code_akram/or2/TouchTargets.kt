package io.github.code_akram.or2

import androidx.compose.ui.test.SemanticsNodeInteraction
import org.junit.Assert.assertTrue

/**
 * Asserts the node's touch bounds (its layout bounds grown to the platform's minimum touch target)
 * are at least [minDp] in both directions: what a finger or an assistive service can actually hit.
 */
fun SemanticsNodeInteraction.assertTouchTargetAtLeast(minDp: Int = 48): SemanticsNodeInteraction {
    val node = fetchSemanticsNode()
    val density = node.layoutInfo.density.density
    val bounds = node.touchBoundsInRoot
    assertTrue("touch width ${bounds.width / density} dp is under $minDp dp", bounds.width >= minDp * density - 1)
    assertTrue("touch height ${bounds.height / density} dp is under $minDp dp", bounds.height >= minDp * density - 1)
    return this
}
