package io.github.code_akram.or2.inbox

import androidx.activity.compose.setContent
import androidx.compose.material3.MaterialTheme
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.getUnclippedBoundsInRoot
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onAllNodesWithTag
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import io.github.code_akram.or2.MainActivity
import io.github.code_akram.or2.connection.uiHost
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.ffi.AgentStatus
import io.github.code_akram.or2.ffi.HerdrAgent
import io.github.code_akram.or2.ffi.HerdrTab
import io.github.code_akram.or2.ffi.HerdrView
import io.github.code_akram.or2.ffi.HerdrWorkspace
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

/** The inbox with fabricated state: no connection, Keystore or database is touched. */
class InboxUiDeviceTest {
    @get:Rule val compose = createAndroidComposeRule<MainActivity>()

    private fun agent(pane: String, status: AgentStatus, name: String = "Claude Code", cwd: String? = "/work/$pane") =
        HerdrAgent(pane, "w1:t1", "w1", name, "claude", name, status, cwd, null, false, 1uL)

    private val view = HerdrView(
        1uL, 22u, null,
        listOf(HerdrWorkspace("w1", 1u, "alpha", true, AgentStatus.BLOCKED)),
        listOf(HerdrTab("w1:t1", "w1", 1u, "editor", true, AgentStatus.BLOCKED)),
        emptyList(),
        listOf(agent("w1:p1", AgentStatus.IDLE), agent("w1:p2", AgentStatus.WORKING, "Codex"), agent("w1:p3", AgentStatus.BLOCKED)),
    )

    private fun row(host: Host, link: LinkStatus, message: String = link.label, note: String? = null, agents: Int = 0) =
        InboxHostRow(host, link, message, note, agents)

    private fun show(
        state: InboxState, busy: Boolean = false, connectAll: () -> Unit = {}, connect: (Host) -> Unit = {},
        openHost: (Host) -> Unit = {}, openAgent: (InboxItem) -> Unit = {},
    ) = compose.runOnUiThread {
        compose.activity.setContent { MaterialTheme { InboxScreen(state, busy, connectAll, connect, openHost, openAgent) } }
    }

    private val box = uiHost(1, "Box")

    @Test
    fun blockedAgentsComeFirstAndEachRowShowsHostAgentPlaceStatusAndCwd() {
        val groups = buildInbox(listOf(InboxSource(1, "Box", null, "default", view)))
        show(InboxState(listOf(row(box, LinkStatus.CONNECTED, "Connected", agents = 3)), groups))
        compose.onNodeWithText("Inbox · 3 agents").assertIsDisplayed()

        fun top(tag: String) = compose.onNodeWithTag(tag).getUnclippedBoundsInRoot().top
        val order = listOf("inbox-group:BLOCKED", "inbox-group:WORKING", "inbox-group:IDLE").map(::top)
        assertEquals(order.sortedBy { it.value }, order)
        assertTrue(order[0] < order[1] && order[1] < order[2])
        val blocked = groups[0].items.single()
        compose.onNodeWithTag(inboxItemTag(blocked)).assertIsDisplayed()
        compose.onNodeWithText("Blocked · 1").assertIsDisplayed()
        compose.onNodeWithText("alpha / editor", substring = true).assertExists()
        compose.onNodeWithText("/work/w1:p3").assertIsDisplayed()
        // One chip per agent, in display order.
        val chips = compose.onAllNodesWithTag("status-chip")
        assertEquals(3, chips.fetchSemanticsNodes().size)
        compose.onNodeWithText("Codex").assertIsDisplayed()
    }

    @Test
    fun tappingARowOpensThatAgent() {
        var opened: InboxItem? = null
        val groups = buildInbox(listOf(InboxSource(1, "Box", "work", "work", view)))
        show(InboxState(listOf(row(box, LinkStatus.CONNECTED)), groups), openAgent = { opened = it })
        compose.onNodeWithTag("inbox-item:1:work:w1:p3").performClick()
        compose.runOnIdle {
            assertEquals("w1:p3", opened!!.paneId)
            assertEquals("work", opened!!.session)
            assertEquals(1L, opened!!.hostId)
        }
    }

    @Test
    fun hostRowsShowConnectionStatusAndOfferConnectOrReview() {
        val other = uiHost(2, "Other")
        val keyless = uiHost(3, "Keyless", keyId = null)
        var connected: Host? = null
        var opened: Host? = null
        show(
            InboxState(listOf(
                row(box, LinkStatus.NOT_CONNECTED),
                row(other, LinkStatus.FAILED, "Authentication rejected. Check the username and public-key authorization."),
                row(keyless, LinkStatus.NOT_CONNECTED),
            ), emptyList()),
            connect = { connected = it }, openHost = { opened = it },
        )
        compose.onNodeWithText("Authentication rejected. Check the username and public-key authorization.").assertIsDisplayed()
        compose.onNodeWithTag("inbox-connect:1").assertIsEnabled().performClick()
        compose.runOnIdle { assertEquals(1L, connected!!.id) }
        compose.onNodeWithText("Retry").assertIsDisplayed() // A failed host offers a retry.
        compose.onNodeWithTag("inbox-connect:3").assertIsNotEnabled() // No key selected: nothing to unlock.
        compose.onNodeWithTag("inbox-open:2").performClick()
        compose.runOnIdle { assertEquals(2L, opened!!.id) }
        // Two or more connectable hosts offer one batch action (one biometric prompt per key).
        compose.onNodeWithTag("inbox-connect-all").assertIsDisplayed()
    }

    @Test
    fun connectAllCallsBackAndIsHiddenWhenThereIsNothingToBatch() {
        var all = 0
        show(InboxState(listOf(row(box, LinkStatus.NOT_CONNECTED), row(uiHost(2, "Two"), LinkStatus.NOT_CONNECTED)), emptyList()),
            connectAll = { all++ })
        compose.onNodeWithTag("inbox-connect-all").performClick()
        compose.runOnIdle { assertEquals(1, all) }

        show(InboxState(listOf(row(box, LinkStatus.NOT_CONNECTED), row(uiHost(2, "Two"), LinkStatus.CONNECTED)), emptyList()))
        compose.onNodeWithTag("inbox-connect-all").assertDoesNotExist()
    }

    @Test
    fun emptyStatesExplainWhatToDo() {
        show(InboxState(emptyList(), emptyList()))
        compose.onNodeWithText("No hosts show agents here.", substring = true).assertIsDisplayed()
        show(InboxState(listOf(row(box, LinkStatus.CONNECTED, note = "No running herdr sessions")), emptyList()))
        compose.onNodeWithText("No running herdr sessions").assertIsDisplayed()
        compose.onNodeWithTag("inbox-empty").assertIsDisplayed()
    }
}
