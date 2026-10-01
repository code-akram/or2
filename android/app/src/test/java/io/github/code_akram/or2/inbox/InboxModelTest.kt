package io.github.code_akram.or2.inbox

import io.github.code_akram.or2.connection.FakePort
import io.github.code_akram.or2.connection.FakeTrust
import io.github.code_akram.or2.connection.HostConnections
import io.github.code_akram.or2.connection.testHost
import io.github.code_akram.or2.data.HostEndpoint
import io.github.code_akram.or2.ffi.AgentStatus
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.HerdrAgent
import io.github.code_akram.or2.ffi.HerdrListener
import io.github.code_akram.or2.ffi.HerdrSessionInfo
import io.github.code_akram.or2.ffi.HerdrState
import io.github.code_akram.or2.ffi.HerdrTab
import io.github.code_akram.or2.ffi.HerdrUnavailable
import io.github.code_akram.or2.ffi.HerdrView
import io.github.code_akram.or2.ffi.HerdrWorkspace
import io.github.code_akram.or2.ffi.HostCapabilities
import io.github.code_akram.or2.ffi.HostListener
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.SessionFailure
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.*
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class InboxModelTest {
    private fun agent(pane: String, status: AgentStatus, workspace: String = "w1", tab: String = "w1:t1", name: String? = "Claude Code",
        cwd: String? = "/work/$pane") =
        HerdrAgent(pane, tab, workspace, name, "claude", name, status, cwd, null, false, 1uL)

    private fun view(vararg agents: HerdrAgent) = HerdrView(
        1uL, 22u, null,
        listOf(HerdrWorkspace("w1", 1u, "alpha", true, AgentStatus.IDLE), HerdrWorkspace("w2", 2u, "beta", false, AgentStatus.IDLE)),
        listOf(HerdrTab("w1:t1", "w1", 1u, "editor", true, AgentStatus.IDLE), HerdrTab("w2:t1", "w2", 1u, "tests", false, AgentStatus.IDLE)),
        emptyList(), agents.toList(),
    )

    private fun source(label: String, id: Long, view: HerdrView, session: String? = null, name: String = session ?: "default") =
        InboxSource(id, label, session, name, view)

    @Test
    fun blockedComesFirstThenWorkingDoneIdleAndEmptyGroupsAreOmitted() {
        val groups = buildInbox(listOf(source("Box", 1, view(
            agent("w1:p1", AgentStatus.IDLE), agent("w1:p2", AgentStatus.DONE), agent("w1:p3", AgentStatus.WORKING),
            agent("w1:p4", AgentStatus.BLOCKED), agent("w1:p5", AgentStatus.UNKNOWN), agent("w1:p6", AgentStatus.WORKING),
        ))))
        assertEquals(listOf(AgentStatus.BLOCKED, AgentStatus.WORKING, AgentStatus.DONE, AgentStatus.IDLE, AgentStatus.UNKNOWN), groups.map { it.status })
        assertEquals(listOf("w1:p3", "w1:p6"), groups[1].items.map { it.paneId })
        assertEquals(listOf(AgentStatus.BLOCKED, AgentStatus.DONE), buildInbox(listOf(source("Box", 1, view(
            agent("w1:p1", AgentStatus.DONE), agent("w1:p2", AgentStatus.BLOCKED))))).map { it.status })
        assertTrue(buildInbox(emptyList()).isEmpty())
        assertTrue(buildInbox(listOf(source("Box", 1, view()))).isEmpty())
    }

    @Test
    fun agentsFromSeveralHostsAndSessionsMergeInsideEachStatusInAStableOrder() {
        val groups = buildInbox(listOf(
            source("Zeta", 2, view(agent("w1:p1", AgentStatus.BLOCKED))),
            source("alpha", 1, view(agent("w2:p1", AgentStatus.BLOCKED, workspace = "w2", tab = "w2:t1"), agent("w1:p9", AgentStatus.BLOCKED)), session = "work"),
            source("alpha", 1, view(agent("w1:p3", AgentStatus.BLOCKED))),
        ))
        val blocked = groups.single().items
        // Host label (case-insensitive), then session, then workspace number, then pane.
        assertEquals(listOf("alpha" to "default", "alpha" to "work", "alpha" to "work", "Zeta" to "default"), blocked.map { it.hostLabel to it.sessionName })
        assertEquals(listOf("w1:p3", "w1:p9", "w2:p1", "w1:p1"), blocked.map { it.paneId })
        // The order does not depend on how the sources arrived.
        val shuffled = buildInbox(listOf(
            source("alpha", 1, view(agent("w1:p3", AgentStatus.BLOCKED))),
            source("Zeta", 2, view(agent("w1:p1", AgentStatus.BLOCKED))),
            source("alpha", 1, view(agent("w2:p1", AgentStatus.BLOCKED, workspace = "w2", tab = "w2:t1"), agent("w1:p9", AgentStatus.BLOCKED)), session = "work"),
        ))
        assertEquals(blocked, shuffled.single().items)
    }

    @Test
    fun rowsCarryHostAgentWorkspaceTabAndCwdAndTheDefaultSessionHasNoName() {
        val item = buildInbox(listOf(
            source("Box", 7, view(agent("w2:p1", AgentStatus.WORKING, workspace = "w2", tab = "w2:t1", cwd = "/home/x/proj")), session = null, name = "default"),
        )).single().items.single()
        assertEquals(7L, item.hostId)
        assertEquals("Box", item.hostLabel)
        assertNull(item.session) // Opened as Herdr(null, pane), never by its listed name.
        assertEquals("default", item.sessionName)
        assertEquals("w2:p1", item.paneId)
        assertEquals("Claude Code", item.agentName)
        assertEquals("beta", item.workspaceLabel)
        assertEquals("tests", item.tabLabel)
        assertEquals("/home/x/proj", item.cwd)
    }

    @Test
    fun agentNamePrefersTheDisplayNameThenNameThenAgentThenAPlaceholder() {
        fun named(display: String?, name: String?, kind: String?) =
            HerdrAgent("p", "t", "w", name, kind, display, AgentStatus.IDLE, null, null, false, 0uL)
        assertEquals("Display", agentName(named("Display", "name", "kind")))
        assertEquals("name", agentName(named(null, "name", "kind")))
        assertEquals("kind", agentName(named(null, null, "kind")))
        assertEquals("agent", agentName(named("", null, null)))
        // A pane whose workspace or tab is unknown still lists, without labels.
        val orphan = buildInbox(listOf(source("Box", 1, HerdrView(1uL, 22u, null, emptyList(), emptyList(), emptyList(), listOf(agent("x:p", AgentStatus.IDLE)))))).single().items.single()
        assertNull(orphan.workspaceLabel)
        assertNull(orphan.tabLabel)
    }

    @Test
    fun linkStatusSummarisesEveryHostState() {
        assertEquals(LinkStatus.NOT_CONNECTED, linkStatus(null))
        assertEquals(LinkStatus.CONNECTING, linkStatus(HostState.Connecting))
        assertEquals(LinkStatus.CONNECTING, linkStatus(HostState.Authenticating))
        assertEquals(LinkStatus.NEEDS_HOST_KEY, linkStatus(HostState.AwaitingHostKeyDecision(
            io.github.code_akram.or2.ffi.PublicKeyInfo("a", "b", "c", ""), emptyList())))
        assertEquals(LinkStatus.CONNECTED, linkStatus(HostState.Connected(1u)))
        assertEquals(LinkStatus.NOT_CONNECTED, linkStatus(HostState.Closed(CloseReason.Disconnected)))
        assertEquals(LinkStatus.FAILED, linkStatus(HostState.Closed(CloseReason.Failed(SessionFailure.TimedOut))))
    }

    @Test
    fun herdrNoteExplainsMissingOrUnavailableHerdr() {
        val caps = HostCapabilities("/t", "/h", null, "C.UTF-8", emptyList())
        assertEquals("Checking the host…", herdrNote(null, null, emptyList()))
        assertTrue(herdrNote(null, "boom", emptyList())!!.startsWith("Could not query"))
        assertEquals("herdr is not installed", herdrNote(caps.copy(herdr = null), null, emptyList()))
        assertEquals("No running herdr sessions", herdrNote(caps, null, emptyList()))
        assertEquals("herdr is not running", herdrNote(caps, null, listOf("default" to HerdrState.Unavailable(HerdrUnavailable.NotRunning, ""))))
        assertEquals("herdr protocol 9 is not supported", herdrNote(caps, null, listOf("default" to HerdrState.Unavailable(HerdrUnavailable.IncompatibleProtocol(9u), ""))))
        // One live session is enough: no note.
        assertNull(herdrNote(caps, null, listOf("a" to HerdrState.Unavailable(HerdrUnavailable.NotRunning, ""), "b" to HerdrState.Live(view()))))
    }

    // --- the flow that assembles it from live connections ---------------------------------------

    @Test
    fun theInboxFollowsConnectionsWatchesAndTheInboxFlag() = runTest {
        val dispatcher = UnconfinedTestDispatcher(testScheduler)
        val ports = mutableMapOf<String, FakePort>()
        val listeners = mutableMapOf<String, HostListener>()
        val holder = HostConnections({ request, listener ->
            listeners[request.addresses[0].host] = listener
            FakePort().also {
                it.caps = HostCapabilities("/t", "/h", null, "C.UTF-8", listOf(HerdrSessionInfo("default", true, true)))
                ports[request.addresses[0].host] = it
            }
        }, FakeTrust(), dispatcher, dispatcher)
        val one = testHost(1, "One", addresses = listOf(HostEndpoint("one.invalid", 22)))
        val two = testHost(2, "Two", addresses = listOf(HostEndpoint("two.invalid", 22)))
        val hidden = testHost(3, "Hidden", addresses = listOf(HostEndpoint("hidden.invalid", 22)), showInInbox = false)
        val hosts = MutableStateFlow(listOf(one, two, hidden))
        val seen = mutableListOf<InboxState>()
        val job = backgroundScope.launch(dispatcher) { holder.inbox(hosts).collect { seen += it } }

        // Nothing connected yet: each inbox host is listed as not connected; the hidden one is not.
        assertEquals(listOf("One", "Two"), seen.last().hosts.map { it.host.label })
        assertTrue(seen.last().hosts.all { it.link == LinkStatus.NOT_CONNECTED })

        holder.connect(listOf(one, two, hidden), byteArrayOf(1))
        listeners["one.invalid"]!!.onHostStateChanged(HostState.Connected(0u))
        listeners["two.invalid"]!!.onHostStateChanged(HostState.Connecting)
        runCurrent()
        assertEquals(listOf(LinkStatus.CONNECTED, LinkStatus.CONNECTING), seen.last().hosts.map { it.link })
        assertNull(seen.last().hosts[0].herdrNote) // The watch has started; nothing to explain yet.

        val watchListener: HerdrListener = ports["one.invalid"]!!.watches.single().second
        watchListener.onHerdrStateChanged(HerdrState.Live(view(
            agent("w1:p1", AgentStatus.IDLE), agent("w1:p2", AgentStatus.BLOCKED))))
        runCurrent()
        val state = seen.last()
        assertEquals(listOf(AgentStatus.BLOCKED, AgentStatus.IDLE), state.groups.map { it.status })
        assertEquals(2, state.agentCount)
        assertEquals(2, state.hosts[0].agentCount)
        assertEquals("One", state.groups[0].items[0].hostLabel)

        // A closed connection stops contributing agents (its watch views are stale).
        listeners["one.invalid"]!!.onHostStateChanged(HostState.Closed(CloseReason.Failed(SessionFailure.ConnectionLost("x"))))
        runCurrent()
        assertEquals(LinkStatus.FAILED, seen.last().hosts[0].link)
        assertTrue(seen.last().groups.isEmpty())

        // The flag is read from the current host list.
        hosts.value = listOf(one, two.copy(record = two.record.copy(showInInbox = false)), hidden.copy(record = hidden.record.copy(showInInbox = true)))
        runCurrent()
        assertEquals(listOf("One", "Hidden"), seen.last().hosts.map { it.host.label })
        job.cancel()
        holder.hosts.value.keys.toList().forEach(holder::dismissHost)
    }

    @Test
    fun linkStatusesAndPendingPromptsFollowTheConnections() = runTest {
        val dispatcher = UnconfinedTestDispatcher(testScheduler)
        val listeners = mutableMapOf<Long, HostListener>()
        val holder = HostConnections({ request, listener ->
            listeners[if (request.addresses[0].host == "one.invalid") 1L else 2L] = listener
            FakePort()
        }, FakeTrust(), dispatcher, dispatcher)
        val one = testHost(1, "One", addresses = listOf(HostEndpoint("one.invalid", 22)))
        val two = testHost(2, "Two", addresses = listOf(HostEndpoint("two.invalid", 22)))
        assertEquals(emptyMap<Long, LinkStatus>(), holder.linkStatuses().first())
        holder.connect(listOf(one, two), byteArrayOf(1))
        val prompt = HostState.AwaitingHostKeyDecision(io.github.code_akram.or2.ffi.PublicKeyInfo("a", "b", "c", ""), emptyList())
        listeners[2]!!.onHostStateChanged(prompt)
        listeners[1]!!.onHostStateChanged(HostState.Connected(0u))
        runCurrent()
        assertEquals(mapOf(1L to LinkStatus.CONNECTED, 2L to LinkStatus.NEEDS_HOST_KEY), holder.linkStatuses().first())
        val pending = holder.pendingHostKeys().first()
        assertEquals(listOf(2L), pending.map { it.active.host.id })
        assertEquals(prompt, pending.single().prompt)
        // One dialog at a time: the shown host's own screen carries its prompt.
        assertEquals(2L, pending.dialogForOtherHost(null)!!.active.host.id)
        assertEquals(2L, pending.dialogForOtherHost(1L)!!.active.host.id)
        assertNull(pending.dialogForOtherHost(2L))
        assertNull(emptyList<PendingHostKey>().dialogForOtherHost(1L))
        holder.hosts.value.keys.toList().forEach(holder::dismissHost)
        assertEquals(emptyList<PendingHostKey>(), holder.pendingHostKeys().first())
    }
}
