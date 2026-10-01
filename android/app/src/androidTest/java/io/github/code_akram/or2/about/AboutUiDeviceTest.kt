package io.github.code_akram.or2.about

import androidx.activity.compose.setContent
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertTextContains
import androidx.compose.ui.test.assertTextEquals
import androidx.compose.ui.test.hasTestTag
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollTo
import androidx.compose.ui.test.performScrollToNode
import io.github.code_akram.or2.MainActivity
import io.github.code_akram.or2.ui.Or2Theme
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test

/** About or2 and the open-source list against the real packaged assets (no network, database or Keystore). */
class AboutUiDeviceTest {
    @get:Rule val compose = createAndroidComposeRule<MainActivity>()

    private val calls = mutableListOf<String>()

    private fun hasTestTagStartingWith(prefix: String) =
        SemanticsMatcher("test tag starts with $prefix") { it.config.getOrNull(SemanticsProperties.TestTag)?.startsWith(prefix) == true }

    @Test
    fun aboutShowsTheVersionsTheLicenceTheSourceAndTheThanks() {
        compose.runOnUiThread {
            compose.activity.setContent {
                Or2Theme {
                    AboutRoute(back = { calls += "back" }, openLicenses = { calls += "licenses" })
                }
            }
        }
        compose.onNodeWithTag("about-version").assertIsDisplayed()
        compose.onNodeWithTag("about-api-version").assertIsDisplayed()
        compose.onNodeWithTag("about-license").assertTextContains(APP_LICENSE, substring = true)
        compose.onNodeWithTag("about-source").assertTextContains(SOURCE_URL, substring = true)
        compose.onNodeWithTag("about-thanks").performScrollTo().assertTextEquals(ACKNOWLEDGEMENT)
        compose.onNodeWithTag("about-licenses").performScrollTo().performClick()
        compose.runOnIdle { assertEquals(listOf("licenses"), calls) }
        // The full GPL text opens from its row and Back returns to the About list.
        compose.onNodeWithTag("about-license").performScrollTo().performClick()
        compose.onNodeWithTag("license-text").assertIsDisplayed()
        compose.onNodeWithTag("top-back").performClick()
        compose.onNodeWithTag("about-list").assertIsDisplayed()
    }

    @Test
    fun theLicenseListShowsAKnownLibraryAndItsFullText() {
        compose.runOnUiThread {
            compose.activity.setContent { Or2Theme { LicensesRoute(back = { calls += "back" }) } }
        }
        compose.waitUntil(10_000) { compose.onAllNodes(hasTestTag("licenses-list")).fetchSemanticsNodes().isNotEmpty() }
        // A Rust crate, an Android artifact and a vendored project, one per group.
        for (prefix in listOf("license:Rust:russh:", "license:Android:androidx.room:room-runtime:", "license:Vendored:mosh-rs:")) {
            compose.onNodeWithTag("licenses-list").performScrollToNode(hasTestTagStartingWith(prefix))
            compose.onNode(hasTestTagStartingWith(prefix)).assertIsDisplayed()
        }
        compose.onNode(hasTestTagStartingWith("license:Rust:russh:")).performClick()
        compose.onNodeWithTag("license-text").assertIsDisplayed()
        compose.onNodeWithTag("top-back").performClick()
        compose.onNodeWithTag("licenses-list").assertIsDisplayed()
    }
}
