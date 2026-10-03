package io.github.code_akram.or2.ui

import android.content.Intent
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.semantics.SemanticsNode
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.test.hasTestTag
import androidx.compose.ui.test.isRoot
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.test.swipeUp
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import androidx.test.core.app.ActivityScenario
import androidx.test.platform.app.InstrumentationRegistry
import io.github.code_akram.or2.gallery.UiGalleryActivity
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

/**
 * The top edge (docs/ui.md, "Top bar" and "Bottom sheets"): on every screen, sheet and dialog of the UI gallery (the
 * real screens with fake data, under the app's own [io.github.code_akram.or2.app.AppScaffold]) nothing is laid out
 * over the status bar or a top cutout; every top bar is one height and stays put while the content scrolls under it,
 * where the scroll edge then shows; and a full-height sheet stops below the status bar.
 */
class TopEdgeDeviceTest {
    @get:Rule val compose = createEmptyComposeRule()

    /** What the status bar and a top cutout take, in pixels, and the screen's density and height. */
    private class Screen(val statusTop: Int, val density: Float, val height: Int)

    private fun <T> gallery(name: String, block: (Screen) -> T): T {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val intent = Intent(context, UiGalleryActivity::class.java).putExtra("screen", name)
        return ActivityScenario.launch<UiGalleryActivity>(intent).use { scenario ->
            var screen: Screen? = null
            scenario.onActivity { activity ->
                val decor = activity.window.decorView
                val insets = ViewCompat.getRootWindowInsets(decor)!!
                    .getInsets(WindowInsetsCompat.Type.statusBars() or WindowInsetsCompat.Type.displayCutout())
                screen = Screen(insets.top, activity.resources.displayMetrics.density, decor.height)
            }
            compose.waitForIdle()
            block(screen!!)
        }
    }

    /** A node's visible bounds on the screen: clipped to its ancestors (a scrolled-away row is empty). */
    private fun SemanticsNode.visibleOnScreen(): Rect {
        val window = boundsInWindow
        val offset = positionOnScreen - positionInWindow
        return window.translate(offset)
    }

    private fun SemanticsNode.describe(): String =
        config.getOrNull(SemanticsProperties.TestTag) ?: config.getOrNull(SemanticsProperties.Text)?.joinToString() ?: "node $id"

    /**
     * Every visible node of every window (the activity, a sheet, a dialog) starts at or below the status bar. A node
     * as large as its whole window is the window itself or a scrim, which is meant to dim the status bar too.
     */
    private fun assertNothingAboveTheStatusBar(screen: Screen, what: String) {
        compose.waitForIdle()
        val roots = compose.onAllNodes(isRoot(), useUnmergedTree = true).fetchSemanticsNodes()
        assertTrue("$what: no compose root", roots.isNotEmpty())
        var checked = 0
        roots.forEach { root ->
            val window = root.boundsInWindow
            fun walk(node: SemanticsNode) {
                val bounds = node.boundsInWindow
                val wholeWindow = bounds.width >= window.width - 1f && bounds.height >= window.height - 1f
                if (bounds.width > 0f && bounds.height > 0f && !wholeWindow) {
                    val top = node.visibleOnScreen().top
                    assertTrue("$what: ${node.describe()} starts at $top px, above the status bar (${screen.statusTop} px)", top >= screen.statusTop - 0.5f)
                    checked++
                }
                node.children.forEach(::walk)
            }
            walk(root)
        }
        assertTrue("$what: nothing was checked", checked > 0)
    }

    private fun top(tag: String): Rect = compose.onNodeWithTag(tag, useUnmergedTree = true).fetchSemanticsNode().visibleOnScreen()

    private fun exists(tag: String) = compose.onAllNodes(hasTestTag(tag), useUnmergedTree = true).fetchSemanticsNodes().isNotEmpty()

    /** Waits until [tag] exists and stops moving (a sheet sliding up). */
    private fun awaitStill(tag: String): Rect {
        var last: Rect? = null
        compose.waitUntil(10_000) {
            if (!exists(tag)) return@waitUntil false
            Thread.sleep(50)
            // Still off screen (an empty, clipped rect) is not still.
            val now = top(tag).takeIf { it.height > 0f }
            (now != null && now == last).also { last = now }
        }
        return last!!
    }

    @Test
    fun noScreenDrawsOverTheStatusBarAndEveryTopBarIsOneHeight() {
        val screens = listOf(
            "home", "home-empty", "home-notices", "host-cards", "inbox", "inbox-empty", "host-form", "host-form-edit",
            "keys", "keys-empty", "settings", "about", "licenses", "license-text", "pair-scan", "pair-scan-denied", "pair-review",
            "pair-progress", "pair-install", "keepalive",
        )
        screens.forEach { name ->
            gallery(name) { screen ->
                if (name == "licenses") compose.waitUntil(10_000) { exists("licenses-list") }
                assertNothingAboveTheStatusBar(screen, name)
                val bar = top("top-bar")
                assertEquals("$name: the top bar sits right under the status bar", screen.statusTop.toFloat(), bar.top, 1f)
                assertEquals("$name: the top bar is ${Or2Dimens.TopBar} tall", Or2Dimens.TopBar.value * screen.density, bar.height, 1f)
                assertTrue("$name: no scroll edge before anything scrolled", !exists("top-bar-edge"))
            }
        }
    }

    @Test
    fun theTopBarStaysPutWhileTheContentScrollsUnderItAndTheEdgeShows() {
        val lists = listOf(
            "host-cards" to "home-list", "inbox" to "inbox-list", "keys" to "keys-list", "about" to "about-list",
            "licenses" to "licenses-list", "license-text" to "license-text", "host-form" to "host-form", "pair-review" to "pair-review",
        )
        lists.forEach { (name, list) ->
            gallery(name) { screen ->
                compose.waitUntil(10_000) { exists(list) }
                val bar = top("top-bar")
                val viewport = top(list)
                // The content starts under the bar, never beside or above it.
                assertTrue("$name: $list starts at ${viewport.top}, the bar ends at ${bar.bottom}", viewport.top >= bar.bottom - 0.5f)
                // A screen whose content fits on this phone does not scroll: the bar must then stay edgeless.
                val range = compose.onNodeWithTag(list).fetchSemanticsNode().config.getOrNull(SemanticsProperties.VerticalScrollAxisRange)
                val scrolls = range != null && range.maxValue() > 0f
                if (name == "licenses" || name == "license-text") assertTrue("$name: $list should be taller than the screen", scrolls)
                compose.onNodeWithTag(list).performTouchInput { swipeUp() }
                compose.waitForIdle()
                assertEquals("$name: the top bar moved while $list scrolled", bar, top("top-bar"))
                assertEquals("$name: the scroll edge shows exactly when $list is scrolled under the bar", scrolls, exists("top-bar-edge"))
                assertNothingAboveTheStatusBar(screen, "$name scrolled")
            }
        }
    }

    @Test
    fun aFullHeightSheetStopsBelowTheStatusBar() {
        listOf("picker-many", "home-picker-many").forEach { name ->
            gallery(name) { screen ->
                val handle = awaitStill("sheet-handle")
                val gap = Or2Dimens.SheetTopGap.value * screen.density
                // It is really full height: thirty sessions do not fit, so the sheet rises as far as it may.
                assertTrue("$name: the sheet is not full height (its top at ${handle.top} px)", handle.top < screen.height * 0.3f)
                assertTrue(
                    "$name: the sheet's top (${handle.top} px) is less than ${Or2Dimens.SheetTopGap} below the status bar (${screen.statusTop} px)",
                    handle.top >= screen.statusTop + gap - 1f,
                )
                assertNothingAboveTheStatusBar(screen, name)
            }
        }
    }

    @Test
    fun noSheetOrDialogCoversTheStatusBar() {
        listOf("home-options", "key-sheet", "shortcuts", "add-host", "picker-herdr", "home-picker", "terminals").forEach { name ->
            gallery(name) { screen ->
                val handle = awaitStill("sheet-handle")
                assertTrue("$name: the sheet's top (${handle.top} px) is over the status bar", handle.top >= screen.statusTop)
                assertNothingAboveTheStatusBar(screen, name)
            }
        }
        listOf("hostkey-first", "hostkey-changed", "hostkey-changed-many").forEach { name ->
            gallery(name) { screen ->
                awaitStill("hostkey-presented")
                assertNothingAboveTheStatusBar(screen, name)
            }
        }
        gallery("inbox-enable-reply") { screen ->
            awaitStill("enable-reply-dialog")
            assertNothingAboveTheStatusBar(screen, "inbox-enable-reply")
        }
    }
}
