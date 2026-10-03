package io.github.code_akram.or2.inbox

import androidx.activity.compose.setContent
import androidx.compose.ui.test.assertCountEquals
import androidx.compose.ui.test.assertHasNoClickAction
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.assertTextEquals
import androidx.compose.ui.test.getUnclippedBoundsInRoot
import androidx.compose.ui.test.hasTestTag
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onAllNodesWithTag
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollToNode
import io.github.code_akram.or2.MainActivity
import io.github.code_akram.or2.connection.uiHost
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.ffi.AgentStatus
import io.github.code_akram.or2.ffi.HerdrAgent
import io.github.code_akram.or2.ffi.HerdrTab
import io.github.code_akram.or2.ffi.HerdrView
import io.github.code_akram.or2.ffi.HerdrWorkspace
import io.github.code_akram.or2.ui.Or2Theme
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

/** The inbox with fabricated state: no connection, Keystore or database is touched. */
class InboxUiDeviceTest {
    @get:Rule val compose = createAndroidComposeRule<MainActivity>()

    private fun agent(pane: String, status: AgentStatus, name: String = "Claude Code", cwd: String? = "/work/$pane") =
        HerdrAgent(pane, "w1:t1", "w1", name, "claude", name, status, cwd, null, false, 1uL, "term_$pane")

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
        openAgent: (InboxItem) -> Unit = {},
    ) = compose.runOnUiThread {
        compose.activity.setContent { Or2Theme { InboxScreen(state, busy, connectAll, connect, openAgent) } }
    }

    /** The host rows sit below the agents; scroll the list to a node that may be off screen. */
    private fun scrollTo(tag: String) = compose.onNodeWithTag("inbox-list").performScrollToNode(hasTestTag(tag))

    private val box = uiHost(1, "Box")

    @Test
    fun blockedAgentsComeFirstAndEachRowShowsHostAgentPlaceStatusAndCwd() {
        val groups = buildInbox(listOf(InboxSource(1, "Box", null, "default", view)))
        show(InboxState(listOf(row(box, LinkStatus.CONNECTED, "Connected", agents = 3)), groups))

        fun top(tag: String) = compose.onNodeWithTag(tag).getUnclippedBoundsInRoot().top
        val order = listOf("inbox-group:BLOCKED", "inbox-group:WORKING", "inbox-group:IDLE").map(::top)
        assertTrue(order[0] < order[1] && order[1] < order[2])
        val blocked = groups[0].items.single()
        compose.onNodeWithTag(inboxItemTag(blocked)).assertIsDisplayed()
        compose.onNodeWithText("BLOCKED · 1").assertIsDisplayed()
        // All three agents are in alpha / editor, so the place line is expected three times.
        compose.onAllNodesWithText("alpha / editor", substring = true).assertCountEquals(3)
        compose.onNodeWithText("/work/w1:p3").assertIsDisplayed()
        // One status label per agent.
        compose.onAllNodesWithTag("status-chip", useUnmergedTree = true).assertCountEquals(3)
        compose.onNodeWithText("Codex").assertIsDisplayed()
        // Blocked rows come with the status in words as well as colour.
        assertEquals("Blocked", groups[0].items.single().status.let(::statusLabel))
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
        show(
            InboxState(listOf(
                row(box, LinkStatus.NOT_CONNECTED),
                row(other, LinkStatus.FAILED, "Authentication rejected. Check the username and public-key authorization."),
                row(keyless, LinkStatus.NOT_CONNECTED),
            ), emptyList()),
            connect = { connected = it },
        )
        scrollTo("inbox-connect:3")
        compose.onNodeWithText("Authentication rejected. Check the username and public-key authorization.").assertIsDisplayed()
        compose.onNodeWithTag("inbox-connect:1").assertIsEnabled().performClick()
        compose.runOnIdle { assertEquals(1L, connected!!.id) }
        compose.onNodeWithText("Retry").assertIsDisplayed() // A failed host offers a retry.
        compose.onNodeWithTag("inbox-connect:1").assertTextEquals("Connect") // The words are Connect and Retry, never Unlock.
        compose.onNodeWithTag("inbox-connect:3").assertIsNotEnabled() // No key selected: nothing to unlock.
        // The row is no link: a tap on it does nothing (Home's card is the host's place); only its pill acts.
        compose.onNodeWithTag("inbox-host:2").assertHasNoClickAction()
        compose.onNodeWithTag("inbox-open:2").assertDoesNotExist()
        // Two or more connectable hosts offer one batch action (one biometric prompt per key).
        compose.onNodeWithTag("inbox-connect-all").assertIsDisplayed()
    }

    @Test
    fun connectAllCallsBackAndIsHiddenWhenThereIsNothingToBatch() {
        var all = 0
        show(InboxState(listOf(row(box, LinkStatus.NOT_CONNECTED), row(uiHost(2, "Two"), LinkStatus.NOT_CONNECTED)), emptyList()),
            connectAll = { all++ })
        scrollTo("inbox-connect-all")
        compose.onNodeWithTag("inbox-connect-all").performClick()
        compose.runOnIdle { assertEquals(1, all) }

        show(InboxState(listOf(row(box, LinkStatus.NOT_CONNECTED), row(uiHost(2, "Two"), LinkStatus.CONNECTED)), emptyList()))
        compose.onNodeWithTag("inbox-connect-all").assertDoesNotExist()
    }

    @Test
    fun emptyStatesExplainWhatToDo() {
        show(InboxState(emptyList(), emptyList()))
        compose.onNodeWithTag("inbox-no-hosts").assertIsDisplayed()
        compose.onNodeWithText("No hosts show agents here.", substring = true).assertIsDisplayed()
        // The same add-host chooser as Home's empty state and its "+" sheet.
        compose.onNodeWithTag("inbox-add-host-chooser").assertExists()
        compose.onNodeWithTag("inbox-add-host-easy").assertExists()
        compose.onNodeWithTag("inbox-add-host-manual").assertExists()
        show(InboxState(listOf(row(box, LinkStatus.CONNECTED, note = "No running herdr sessions")), emptyList()))
        compose.onNodeWithTag("inbox-empty").assertIsDisplayed()
        compose.onNodeWithText("No agent events yet").assertIsDisplayed()
        compose.onNodeWithText("No running herdr sessions").assertIsDisplayed()
    }

    @Test
    fun herdrsOwnExplanationShowsInMutedTextOnTheHostRow() {
        val note = "herdr is unavailable: the session's socket cannot be opened"
        show(InboxState(listOf(row(box, LinkStatus.CONNECTED, note = note)), emptyList()))
        compose.onNodeWithTag("inbox-herdr-note:1", useUnmergedTree = true).assertIsDisplayed()
        compose.onNodeWithText(note).assertIsDisplayed()
    }

    @Test
    fun aSleepingHostReadsAsleepInMutedTextAndCanStillBeUnlocked() {
        var connected: Host? = null
        show(InboxState(listOf(row(uiHost(1, "MacBook", sleeps = true), LinkStatus.ASLEEP, "Asleep")), emptyList()), connect = { connected = it })
        compose.onNodeWithText("Asleep").assertIsDisplayed()
        compose.onNodeWithText("Retry").assertDoesNotExist() // Not a failure to retry.
        compose.onNodeWithTag("inbox-connect:1").assertIsEnabled().performClick() // The user knows it woke up.
        compose.runOnIdle { assertEquals(1L, connected!!.id) }
    }
}
