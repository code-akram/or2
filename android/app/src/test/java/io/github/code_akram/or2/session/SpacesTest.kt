package io.github.code_akram.or2.session

import io.github.code_akram.or2.ffi.AgentStatus
import io.github.code_akram.or2.ffi.HerdrAgent
import io.github.code_akram.or2.ffi.HerdrPane
import io.github.code_akram.or2.ffi.HerdrTab
import io.github.code_akram.or2.ffi.HerdrView
import io.github.code_akram.or2.ffi.HerdrWorkspace
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** The Spaces sheet's grouping: spaces, their tabs and their agents in herdr's order, and the focused tab and pane. */
class SpacesTest {
    private fun agent(pane: String, tab: String, status: AgentStatus, name: String? = null, display: String? = "Claude Code", title: String? = null) =
        HerdrAgent(pane, tab, tab.substringBefore(':'), name, "claude", display, status, null, 1uL, "term_$pane", null, title)

    /** Listed out of herdr's order on purpose: the sheet follows herdr's numbers. */
    private fun view(focusedPane: String? = "w2:p2", focusedTab: String? = "w2:t1") = HerdrView(
        1uL, focusedPane,
        listOf(HerdrWorkspace("w2", 2u, "or2"), HerdrWorkspace("w1", 1u, "~"), HerdrWorkspace("w3", 3u, "docs")),
        listOf(HerdrTab("w2:t2", "w2", 2u, "2"), HerdrTab("w2:t1", "w2", 1u, "ui"), HerdrTab("w1:t1", "w1", 1u, "1")),
        // herdr's pane order: p2 before p1 in tab ui.
        listOf(HerdrPane("w2:p2", "claude", null), HerdrPane("w2:p1", "claude", null), HerdrPane("w2:p3", "codex", null)),
        listOf(
            agent("w2:p1", "w2:t1", AgentStatus.BLOCKED, title = "Repository context gathering"),
            agent("w2:p2", "w2:t1", AgentStatus.WORKING, name = "reviewer", title = "Review v013 brief | or2"),
            agent("w2:p3", "w2:t2", AgentStatus.DONE, display = "Codex", title = "Codex"),
        ),
        focusedTab,
    )

    @Test
    fun spacesTabsAndAgentsFollowHerdrsOrderAndSpacesWithoutTabsAreLeftOut() {
        val spaces = spacesOf(view())
        assertEquals(listOf("~", "or2"), spaces.map { it.label })
        assertEquals(listOf(listOf("w1:t1"), listOf("w2:t1", "w2:t2")), spaces.map { space -> space.tabs.map { it.tabId } })
        assertEquals(listOf("tab 1", "tab ui", "tab 2"), spaces.flatMap { it.tabs }.map(::tabRowLabel))
        val ui = spaces[1].tabs[0]
        assertEquals(listOf("w2:p2", "w2:p1"), ui.agents.map { it.paneId })
        assertTrue(spaces[0].tabs[0].agents.isEmpty())
    }

    @Test
    fun agentRowsAreNamedHerdrStyleWithTheirTitleAndStatus() {
        val agents = spacesOf(view()).flatMap { it.tabs }.flatMap { it.agents }.associateBy { it.paneId }
        // Started by name: the name; else the display name. The title is its own line unless it repeats the label.
        assertEquals(SpaceAgent("w2:p2", "reviewer", "Review v013 brief | or2", AgentStatus.WORKING, current = true), agents["w2:p2"])
        assertEquals(SpaceAgent("w2:p1", "Claude Code", "Repository context gathering", AgentStatus.BLOCKED, current = false), agents["w2:p1"])
        assertEquals(SpaceAgent("w2:p3", "Codex", null, AgentStatus.DONE, current = false), agents["w2:p3"])
    }

    @Test
    fun theFocusedTabAndPaneAreCurrent() {
        fun current(view: HerdrView) = spacesOf(view).flatMap { it.tabs }.let { tabs ->
            tabs.filter { it.current }.map { it.tabId } to tabs.flatMap { it.agents }.filter { it.current }.map { it.paneId }
        }
        assertEquals(listOf("w2:t1") to listOf("w2:p2"), current(view()))
        // A tab with no agent focused: the tab is current, no agent is.
        assertEquals(listOf("w1:t1") to emptyList<String>(), current(view(focusedPane = "w1:p1", focusedTab = "w1:t1")))
        // A herdr that names no focused tab: the focused pane's tab.
        assertEquals(listOf("w2:t2") to listOf("w2:p3"), current(view(focusedPane = "w2:p3", focusedTab = null)))
        assertEquals(emptyList<String>() to emptyList<String>(), current(view(focusedPane = null, focusedTab = null)))
    }

    @Test
    fun anEmptyViewHasNoSpaces() {
        assertTrue(spacesOf(HerdrView(1uL, null, emptyList(), emptyList(), emptyList(), emptyList())).isEmpty())
    }
}
