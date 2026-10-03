package io.github.code_akram.or2.session

import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.test.assertContentDescriptionEquals
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertTextEquals
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onAllNodesWithTag
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import io.github.code_akram.or2.MainActivity
import io.github.code_akram.or2.app.AppScaffold
import io.github.code_akram.or2.assertTouchTargetAtLeast
import io.github.code_akram.or2.connection.HostConnections
import io.github.code_akram.or2.connection.UiPort
import io.github.code_akram.or2.connection.UiTrust
import io.github.code_akram.or2.connection.uiHost
import io.github.code_akram.or2.ffi.AgentStatus
import io.github.code_akram.or2.ffi.HerdrAgent
import io.github.code_akram.or2.ffi.HerdrState
import io.github.code_akram.or2.ffi.HerdrTab
import io.github.code_akram.or2.ffi.HerdrView
import io.github.code_akram.or2.ffi.HerdrWorkspace
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.terminal.Transport
import io.github.code_akram.or2.ui.Or2Theme
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

/** The Spaces disc (herdr terminals only, matching the other two), its sheet, and a tap's focus through herdr. */
class SpacesDeviceTest {
    @get:Rule val compose = createAndroidComposeRule<MainActivity>()

    private fun bounds(tag: String): Rect = compose.onNodeWithTag(tag).fetchSemanticsNode().boundsInRoot

    private fun waitFor(tag: String, unmerged: Boolean = false) = compose.waitUntil(5_000) {
        compose.onAllNodesWithTag(tag, useUnmergedTree = unmerged).fetchSemanticsNodes().isNotEmpty()
    }

    @Test
    fun theThirdDiscIsOnlyOnHerdrTerminalsAndMatchesTheOtherTwoWhichDoNotMove() {
        var spaces by mutableStateOf<(() -> Unit)?>(null)
        var opened = 0
        compose.runOnUiThread {
            compose.activity.setContent {
                Or2Theme {
                    TerminalCard(
                        "workstation", "tmux main", Transport.MOSH, SessionState.Connected, minimise = {}, openSwitcher = {}, endSession = {},
                        openSpaces = spaces,
                    ) { Box(Modifier.weight(1f).fillMaxWidth().testTag("terminal-body")) }
                }
            }
        }
        // Shell and tmux: two discs, as before.
        compose.onNodeWithTag("terminal-spaces").assertDoesNotExist()
        val back = bounds("terminal-back")
        val panes = bounds("terminal-panes")
        // herdr: a third disc right after them, the same size and step, the first two where they were.
        compose.runOnUiThread { spaces = { opened++ } }
        compose.onNodeWithTag("terminal-spaces").assertIsDisplayed().assertContentDescriptionEquals("Spaces")
        val disc = bounds("terminal-spaces")
        assertEquals(back, bounds("terminal-back"))
        assertEquals(panes, bounds("terminal-panes"))
        assertEquals(panes.width, disc.width, 0.5f)
        assertEquals(panes.height, disc.height, 0.5f)
        assertEquals("one step apart", panes.left - back.left, disc.left - panes.left, 1f)
        assertEquals("boxes meet halfway", panes.right, disc.left, 1f)
        compose.onNodeWithTag("terminal-spaces").assertTouchTargetAtLeast(48)
        // The title stays clear of all three.
        val title = compose.onNodeWithTag("terminal-title", useUnmergedTree = true).fetchSemanticsNode().boundsInRoot
        assertTrue("title $title overlaps the discs", title.left >= disc.right)
        compose.onNodeWithTag("terminal-spaces").performClick()
        compose.runOnIdle { assertEquals(1, opened) }
    }

    private fun terminalHolder(port: UiPort): HostConnections {
        val holder = HostConnections({ _, listener -> port.also { it.hostListener = listener } }, UiTrust(), worker = Dispatchers.Unconfined)
        compose.runOnUiThread {
            runBlocking { holder.connect(uiHost(), byteArrayOf(1)) }
            port.native = HostState.Connected(0u)
        }
        return holder
    }

    /** The default session: `~` with one tab, `or2` with `ui` (two agents, the second focused) and `2` (none). */
    private val view = HerdrView(
        1uL, "w2:p2",
        listOf(HerdrWorkspace("w1", 1u, "~"), HerdrWorkspace("w2", 2u, "or2"), HerdrWorkspace("w3", 3u, "empty")),
        listOf(HerdrTab("w1:t1", "w1", 1u, "1"), HerdrTab("w2:t1", "w2", 1u, "ui"), HerdrTab("w2:t2", "w2", 2u, "2")),
        emptyList(),
        listOf(
            HerdrAgent("w2:p1", "w2:t1", "w2", null, "claude", "Claude Code", AgentStatus.BLOCKED, null, 1uL, "term_1", null,
                "Repository context gathering"),
            HerdrAgent("w2:p2", "w2:t1", "w2", "reviewer", "codex", "Codex", AgentStatus.WORKING, null, 1uL, "term_2", null, null),
        ),
        "w2:t1",
    )

    @Test
    fun theSheetListsTheSessionsSpacesAndATapFocusesThroughHerdrAndCloses() {
        val port = UiPort()
        val holder = terminalHolder(port)
        // The default session's watch (the host shows in the inbox) reports the view, as Rust would.
        compose.waitUntil(5_000) { port.watchListeners.isNotEmpty() }
        compose.runOnUiThread { port.watchListeners.first().onHerdrStateChanged(HerdrState.Live(view)) }
        compose.runOnUiThread {
            val terminal = holder.openTerminal(holder.host(1)!!, TerminalTarget.Herdr(null, null))
            compose.activity.setContent {
                AppScaffold(fullScreen = true) { SessionScreen(holder, terminal, listOf(terminal), minimise = {}, select = {}) }
            }
        }
        waitFor("terminal-spaces")
        compose.onNodeWithTag("terminal-spaces").performClick()
        waitFor("spaces-sheet")
        // The default session has no name under the title.
        compose.onNodeWithTag("sheet-subtitle", useUnmergedTree = true).assertDoesNotExist()
        compose.onNodeWithTag("space:w1").assertIsDisplayed()
        compose.onNodeWithTag("space:w3").assertDoesNotExist() // No tabs: left out.
        compose.onNodeWithTag("space-label:w2", useUnmergedTree = true).assertTextEquals("or2")
        compose.onNodeWithTag("space-tab-current:w2:t1", useUnmergedTree = true).assertIsDisplayed()
        compose.onNodeWithTag("space-tab-current:w2:t2", useUnmergedTree = true).assertDoesNotExist()
        compose.onNodeWithTag("space-agent-current:w2:p2", useUnmergedTree = true).assertIsDisplayed()
        compose.onNodeWithTag("space-agent-current:w2:p1", useUnmergedTree = true).assertDoesNotExist()
        compose.onNodeWithTag("space-agent-title:w2:p1", useUnmergedTree = true).assertTextEquals("Repository context gathering")
        compose.onNodeWithTag("space-agent-status:w2:p1", useUnmergedTree = true).assertTextEquals("Blocked")

        // A tab: herdr's tab focus in the terminal's session, and the sheet closes.
        compose.onNodeWithTag("space-tab:w2:t2").performClick()
        compose.waitUntil(5_000) { port.focusedTabs.isNotEmpty() }
        compose.onNodeWithTag("spaces-sheet").assertDoesNotExist()
        compose.runOnIdle { assertEquals(listOf<Pair<String?, String>>(null to "w2:t2"), port.focusedTabs) }

        // An agent: its pane is focused.
        compose.onNodeWithTag("terminal-spaces").performClick()
        waitFor("spaces-sheet")
        compose.onNodeWithTag("space-agent:w2:p1").performClick()
        compose.waitUntil(5_000) { port.focused.isNotEmpty() }
        compose.onNodeWithTag("spaces-sheet").assertDoesNotExist()
        compose.runOnIdle {
            assertEquals(listOf<Pair<String?, String>>(null to "w2:p1"), port.focused)
            assertEquals(1, port.sessions.size) // Nothing else opened.
        }
    }

    @Test
    fun aShellTerminalHasNoSpacesDisc() {
        val port = UiPort()
        val holder = terminalHolder(port)
        compose.runOnUiThread {
            val terminal = holder.openTerminal(holder.host(1)!!, TerminalTarget.Shell)
            compose.activity.setContent {
                AppScaffold(fullScreen = true) { SessionScreen(holder, terminal, listOf(terminal), minimise = {}, select = {}) }
            }
        }
        waitFor("terminal-panes")
        compose.onNodeWithTag("terminal-spaces").assertDoesNotExist()
    }
}
