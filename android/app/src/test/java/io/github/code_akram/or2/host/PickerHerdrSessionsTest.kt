package io.github.code_akram.or2.host

import io.github.code_akram.or2.ffi.AgentStatus
import io.github.code_akram.or2.ffi.HerdrAgent
import io.github.code_akram.or2.ffi.HerdrPane
import io.github.code_akram.or2.ffi.HerdrSessionInfo
import io.github.code_akram.or2.ffi.HerdrTab
import io.github.code_akram.or2.ffi.HerdrView
import io.github.code_akram.or2.ffi.HerdrWorkspace
import io.github.code_akram.or2.inbox.statusColor
import io.github.code_akram.or2.inbox.statusLabel
import io.github.code_akram.or2.ui.Or2Colors
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The herdr tab lists each running session's agents from the host's live views (the Inbox's source), grouped by
 * workspace; a session without a live view is one row, as before.
 */
class PickerHerdrSessionsTest {
    private fun agent(
        pane: String, workspace: String = "w1", tab: String = "w1:t1", status: AgentStatus = AgentStatus.IDLE,
        display: String? = "Claude Code", cwd: String? = "~/code/or2",
    ) = HerdrAgent(pane, tab, workspace, null, null, display, status, cwd, 1uL, "term_$pane")

    private fun view(vararg agents: HerdrAgent, panes: List<HerdrPane> = emptyList()) = HerdrView(
        1uL, null,
        // Listed out of order: the picker follows herdr's numbers.
        listOf(HerdrWorkspace("w2", 2u, "docs"), HerdrWorkspace("w1", 1u, "or2"), HerdrWorkspace("w3", 3u, "empty")),
        listOf(HerdrTab("w1:t2", "w1", 2u, "tests"), HerdrTab("w1:t1", "w1", 1u, "ui"), HerdrTab("w2:t1", "w2", 1u, "readme")),
        panes, agents.toList(),
    )

    private val default = HerdrSessionInfo("default", true, true)
    private val spike = HerdrSessionInfo("or2-spike", true, false)
    private val archive = HerdrSessionInfo("archive", false, false)

    @Test
    fun agentsAreGroupedBySessionThenWorkspaceInHerdrsOrder() {
        val sessions = pickerHerdrSessions(listOf(default, spike), mapOf(
            null to view(
                agent("w2:p1", workspace = "w2", tab = "w2:t1", display = "Amp", status = AgentStatus.WORKING),
                agent("w1:p3", tab = "w1:t2", status = AgentStatus.DONE),
                agent("w1:p2", status = AgentStatus.BLOCKED),
                agent("w1:p1"),
            ),
            "or2-spike" to view(agent("w1:p1", display = "Codex")),
        ))
        assertEquals(listOf("default", "or2-spike"), sessions.map { it.label })
        val workspaces = sessions[0].workspaces!!
        // Workspace 1 before 2; one without agents is left out. Within one: tab, then pane.
        assertEquals(listOf("or2", "docs"), workspaces.map { it.label })
        assertEquals(listOf("w1:p1", "w1:p2", "w1:p3"), workspaces[0].agents.map { it.paneId })
        assertEquals(PickerAgent("w2:p1", "Amp", AgentStatus.WORKING, "~/code/or2"), workspaces[1].agents.single())
        assertEquals(listOf("Codex"), sessions[1].workspaces!!.single().agents.map { it.label })
    }

    @Test
    fun theDefaultSessionIsLabelledByItsNameAndOpensWithoutOne() {
        val sessions = pickerHerdrSessions(listOf(default, spike), mapOf(null to view(), "or2-spike" to view()))
        assertEquals("default", sessions[0].label) // No "(default)" suffix.
        assertNull(sessions[0].session) // What TerminalTarget.Herdr and an agent's open take for it.
        assertEquals("or2-spike", sessions[1].session)
        // The default session's view is keyed null, never by its listed name.
        assertFalse(pickerHerdrSessions(listOf(default), mapOf("default" to view(agent("w1:p1"))))[0].live)
    }

    @Test
    fun aLiveSessionWithoutAgentsHasAnEmptyListAndOneWithoutAViewIsOneRow() {
        val sessions = pickerHerdrSessions(listOf(archive, spike, default), mapOf("or2-spike" to view()))
        // The live ones first, then the others in the listing's order.
        assertEquals(listOf("or2-spike", "archive", "default"), sessions.map { it.label })
        assertEquals(emptyList<PickerWorkspace>(), sessions[0].workspaces) // "No agents" under Whole session.
        assertNull(sessions[1].workspaces) // Not running: one row.
        assertFalse(sessions[1].running)
        assertNull(sessions[2].workspaces) // Running but not watched (yet): one row too.
        assertTrue(sessions[2].running)
    }

    @Test
    fun aLiveViewMeansRunningWhateverTheCachedProbeSaid() {
        val started = pickerHerdrSessions(listOf(archive), mapOf("archive" to view(agent("w1:p1"))))[0]
        assertTrue(started.live)
        assertTrue(started.running)
    }

    @Test
    fun anAgentIsLabelledAsTheInboxLabelsItWithThePanesNameAndCwdAsFallbacks() {
        val unnamed = agent("w1:p1", display = null, cwd = null)
        val panes = listOf(HerdrPane("w1:p1", "pi", "/srv/work"))
        assertEquals(PickerAgent("w1:p1", "pi", AgentStatus.IDLE, "/srv/work"), pickerWorkspaces(view(unnamed, panes = panes))[0].agents[0])
        assertEquals("agent", pickerWorkspaces(view(unnamed))[0].agents[0].label)
        assertNull(pickerWorkspaces(view(unnamed))[0].agents[0].cwd)
    }

    @Test
    fun agentsInNoListedWorkspaceComeLastWithoutAHeader() {
        val groups = pickerWorkspaces(view(agent("w9:p1", workspace = "w9"), agent("w1:p1")))
        assertEquals(listOf("or2", null), groups.map { it.label })
        assertEquals("w9:p1", groups[1].agents.single().paneId)
    }

    @Test
    fun statusDotsAndWordsAreTheInboxs() {
        assertEquals(listOf("Working", "Blocked", "Done", "Idle"),
            listOf(AgentStatus.WORKING, AgentStatus.BLOCKED, AgentStatus.DONE, AgentStatus.IDLE).map(::statusLabel))
        assertEquals(Or2Colors.Working, statusColor(AgentStatus.WORKING))
        assertEquals(Or2Colors.Attention, statusColor(AgentStatus.BLOCKED))
        assertEquals(Or2Colors.Done, statusColor(AgentStatus.DONE))
        assertEquals(Or2Colors.Idle, statusColor(AgentStatus.IDLE))
        // Carried from the view as it is.
        val statuses = AgentStatus.entries.mapIndexed { index, status -> agent("w1:p$index", status = status) }
        assertEquals(AgentStatus.entries.toList(), pickerWorkspaces(view(*statuses.toTypedArray()))[0].agents.map { it.status })
    }
}
