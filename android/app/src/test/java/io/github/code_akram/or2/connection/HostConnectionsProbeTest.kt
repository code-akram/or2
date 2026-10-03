package io.github.code_akram.or2.connection

import io.github.code_akram.or2.app.MemoryPrefStore
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.HostEndpoint
import io.github.code_akram.or2.data.TrustStore
import io.github.code_akram.or2.ffi.AgentStatus
import io.github.code_akram.or2.data.TransportPref
import io.github.code_akram.or2.ffi.ClientKeyMaterial
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.LinkHealth
import io.github.code_akram.or2.ffi.TerminalTransport
import io.github.code_akram.or2.ffi.networkChanged
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import io.github.code_akram.or2.ffi.HerdrState
import io.github.code_akram.or2.ffi.HostException
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.PublicKeyInfo
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalFrame
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.ffi.contractProbeHost
import io.github.code_akram.or2.ffi.generateEd25519Key
import io.github.code_akram.or2.inbox.INBOX_STATUS_ORDER
import io.github.code_akram.or2.inbox.herdrViews
import io.github.code_akram.or2.inbox.inbox
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import org.junit.Assert.*
import org.junit.Test
import java.util.concurrent.Executors

/**
 * The holder end to end over the real FFI: `contractProbeHost` stands in for `connect_host`
 * (scripted host, no network), so host-key relay, trust persistence, the capability probe, herdr
 * watches feeding the inbox, terminals and disconnect ordering run against real Rust callbacks
 * on Rust threads, delivered through the holder's main dispatcher.
 */
class HostConnectionsProbeTest {
    private class Store : TrustStore {
        var lines = emptyList<String>()
        var replacements = 0
        override suspend fun trustedKeys(hostId: Long) = lines
        override suspend fun replaceTrust(host: Host, presented: PublicKeyInfo) {
            lines = listOf(presented.openssh)
            replacements++
        }
    }

    private val probe = HostConnector { request, listener -> NativeHostPort(contractProbeHost(request, listener)) }

    private fun TerminalFrame.row0() = changedRows.single { it.index.toInt() == 0 }.cells.joinToString("") { it.text }.trimEnd()

    @Test
    fun hostKeyCapabilitiesInboxTerminalsAndDisconnectThroughTheHolder() = runBlocking {
        Executors.newSingleThreadExecutor().asCoroutineDispatcher().use { main ->
            withContext(main) {
                val store = Store()
                val holder = HostConnections(probe, store, main)
                val key = generateEd25519Key("probe")
                val host = testHost(addresses = listOf(HostEndpoint("probe.invalid", 22)))
                val bytes = key.privateKey.copyOf()
                try {
                    holder.connect(host, bytes)
                    assertTrue(bytes.all { it == 0.toByte() })
                    val active = holder.host(host.id)!!

                    // First use: the prompt, bound to the presented key, persisted before approval.
                    val prompt = withTimeout(5000) { active.state.first { it is HostState.AwaitingHostKeyDecision } }
                        as HostState.AwaitingHostKeyDecision
                    assertTrue(prompt.previouslyTrusted.isEmpty())
                    holder.approve(active, prompt)
                    assertEquals(listOf(prompt.presented.openssh), store.lines)
                    assertEquals(1, store.replacements)
                    assertEquals(HostState.Connected(0u), withTimeout(5000) { active.state.first { it is HostState.Connected } })
                    assertTrue(active.hasConnected.value)

                    // The capability probe runs once connected; the default and every listed session are watched.
                    val caps = withTimeout(5000) { active.capabilities.first { it != null } }!!
                    assertEquals("/usr/bin/tmux", caps.tmux)
                    assertEquals(listOf("default", "or2-probe"), caps.herdrSessions.map { it.name })
                    assertEquals(listOf<String?>(null, "or2-probe"), active.watches.value.map { it.session }) // Default is unnamed.
                    assertEquals(listOf("main", "build"), holder.listTmuxSessions(active).map { it.name })

                    // Agents of both sessions (3 each) reach the inbox, most urgent status first.
                    val inbox = withTimeout(5000) { holder.inbox(flowOf(listOf(host))).first { it.agentCount == 6 } }
                    val order = inbox.groups.map { it.status }
                    assertEquals(order.sortedBy { INBOX_STATUS_ORDER.indexOf(it) }, order)
                    assertTrue(AgentStatus.IDLE in order)
                    assertEquals(host.label, inbox.groups.first().items.first().hostLabel)
                    assertEquals(inbox.hosts.single().agentCount, 6)
                    val pane = inbox.groups.flatMap { it.items }.first { it.session == null }
                    assertNull(pane.session) // Default session: opened without a name.
                    assertEquals(setOf<String?>(null, "or2-probe"), inbox.groups.flatMap { it.items }.map { it.session }.toSet())
                    withTimeout(5000) { active.watches.value.forEach { it.state.first { state -> state is HerdrState.Live } } }
                    // Home's session cards read every live view by host and session, whatever the inbox shows.
                    val views = withTimeout(5000) { holder.herdrViews().first { it.size == 2 } }
                    assertEquals(setOf<Pair<Long, String?>>(host.id to null, host.id to "or2-probe"), views.keys)
                    assertNotNull(views.getValue(host.id to null).focusedPaneId)

                    // Terminals: several on the one connection, each with its own lifecycle and frames.
                    val shell = holder.openTerminal(active, TerminalTarget.Shell)
                    val herdr = holder.openTerminal(active, TerminalTarget.Herdr(pane.session, pane.paneId))
                    withTimeout(5000) { shell.state.first { it == SessionState.Connected } }
                    withTimeout(5000) { herdr.state.first { it == SessionState.Connected } }
                    withTimeout(5000) { shell.frameReady.first() }
                    withTimeout(5000) { herdr.frameReady.first() }
                    assertEquals("or2 contract probe shell", shell.handle.value!!.takeFrame()!!.row0())
                    assertEquals("or2 contract probe herdr default ${pane.paneId}", herdr.handle.value!!.takeFrame()!!.row0())
                    holder.disconnectTerminal(shell)
                    withTimeout(5000) { shell.state.first { it is SessionState.Closed } }
                    assertEquals(SessionState.Closed(CloseReason.Disconnected), shell.state.value)
                    assertEquals(HostState.Connected(0u), active.state.value) // Only that channel ended.
                    assertEquals(SessionState.Connected, herdr.state.value)

                    // Disconnecting the host closes the rest, then reports its own Closed.
                    holder.disconnect(host.id)
                    withTimeout(5000) { active.state.first { it is HostState.Closed } }
                    assertEquals(HostState.Closed(CloseReason.Disconnected), active.state.value)
                    assertEquals(SessionState.Closed(CloseReason.Disconnected), withTimeout(5000) { herdr.state.first { it is SessionState.Closed } })
                    active.watches.value.forEach { assertEquals(HerdrState.Closed, withTimeout(5000) { it.state.first { state -> state == HerdrState.Closed } }) }
                    assertEquals(HostState.Closed(CloseReason.Disconnected), active.mutablePort.value!!.state()) // Not destroyed yet.

                    holder.dismissTerminal(shell)
                    holder.dismissTerminal(herdr)
                    holder.dismissHost(host.id)
                    assertThrows(IllegalStateException::class.java) { active.mutablePort.value!!.state() }
                    assertTrue(holder.terminals.value.isEmpty())
                    assertEquals(1, store.replacements)
                } finally { bytes.fill(0); key.privateKey.fill(0); holder.release(host.id, closeTerminals = true) }
            }
        }
    }

    @Test
    fun aTrustedProbeHostConnectsWithoutAPromptAndAChangedKeyIsRejectable() = runBlocking {
        Executors.newSingleThreadExecutor().asCoroutineDispatcher().use { main ->
            withContext(main) {
                val store = Store()
                val holder = HostConnections(probe, store, main)
                val key = generateEd25519Key("probe")
                val host = testHost(addresses = listOf(HostEndpoint("probe.invalid", 22)))
                try {
                    holder.connect(host, key.privateKey.copyOf())
                    val first = holder.host(host.id)!!
                    val prompt = withTimeout(5000) { first.state.first { it is HostState.AwaitingHostKeyDecision } }
                        as HostState.AwaitingHostKeyDecision
                    holder.approve(first, prompt)
                    withTimeout(5000) { first.state.first { it is HostState.Connected } }
                    holder.disconnect(host.id)
                    withTimeout(5000) { first.state.first { it is HostState.Closed } }

                    holder.connect(host, key.privateKey.copyOf()) // The stored key now matches.
                    val trusted = holder.host(host.id)!!
                    assertNotSame(first, trusted)
                    withTimeout(5000) { trusted.state.first { it is HostState.Connected } }
                    assertEquals(1, store.replacements) // No new prompt, no new write.
                    holder.disconnect(host.id)
                    withTimeout(5000) { trusted.state.first { it is HostState.Closed } }

                    val unrelated = generateEd25519Key("old host key")
                    try { store.lines = listOf(unrelated.publicKey.openssh) } finally { unrelated.privateKey.fill(0) }
                    holder.connect(host, key.privateKey.copyOf())
                    val changed = holder.host(host.id)!!
                    val changedPrompt = withTimeout(5000) { changed.state.first { it is HostState.AwaitingHostKeyDecision } }
                        as HostState.AwaitingHostKeyDecision
                    assertEquals(1, changedPrompt.previouslyTrusted.size)
                    holder.reject(changed)
                    withTimeout(5000) { changed.state.first { it is HostState.Closed } }
                    assertEquals(1, store.replacements) // Reject never overwrites the old trust.
                } finally { key.privateKey.fill(0); holder.release(host.id, closeTerminals = true) }
            }
        }
    }

    private fun TerminalFrame.rowText(index: Int) =
        changedRows.single { it.index.toInt() == index }.cells.joinToString("") { it.text }.trimEnd()

    /** Connects [host] to the probe and returns its connection, probed and connected. */
    private suspend fun connectedProbe(holder: HostConnections, host: Host, key: ClientKeyMaterial): ActiveHost {
        holder.connect(host, key.privateKey.copyOf())
        val active = holder.host(host.id)!!
        val prompt = withTimeout(5000) { active.state.first { it is HostState.AwaitingHostKeyDecision } } as HostState.AwaitingHostKeyDecision
        holder.approve(active, prompt)
        withTimeout(5000) { active.state.first { it is HostState.Connected } }
        withTimeout(5000) { active.capabilities.first { it != null } }
        return active
    }

    @Test
    fun autoOpensTmuxOverSshThenSwapsToMoshAgainstTheProbeWithItsHealthSequenceAndRoamsOnNetworkChanged() = runBlocking<Unit> {
        Executors.newSingleThreadExecutor().asCoroutineDispatcher().use { main ->
            withContext(main) {
                val store = Store()
                val holder = HostConnections(probe, store, main)
                val key = generateEd25519Key("probe")
                val host = testHost(addresses = listOf(HostEndpoint("probe.invalid", 22)))
                try {
                    val active = connectedProbe(holder, host, key)
                    assertEquals("/usr/bin/mosh-server", active.capabilities.value!!.moshServer)

                    val health = mutableListOf<LinkHealth>()
                    val mosh = holder.openTerminal(active, TerminalTarget.Tmux("work"))
                    val collector = launch { mosh.linkHealth.collect { it?.let(health::add) } }
                    // AUTO with UDP untested: SSH at once, and the real background mosh session swapped in.
                    assertEquals(TerminalTransport.SSH, mosh.transport.value)
                    val ssh = mosh.handle.value!!
                    withTimeout(5000) { mosh.transport.first { it == TerminalTransport.MOSH } }
                    assertEquals(TerminalTransport.MOSH, mosh.handle.value!!.transport())
                    assertNotSame(ssh, mosh.handle.value)
                    assertEquals(UdpVerdict.OK, active.udpVerdict.value)
                    withTimeout(5000) { mosh.state.first { it == SessionState.Connected } }
                    withTimeout(5000) { mosh.linkHealth.first { it?.sinceHeardMs == 400uL } }
                    collector.cancel()
                    // `linkHealth` is a StateFlow: it keeps the latest value, and the probe reports all
                    // three back to back, so a collector that is scheduled late (a loaded machine) sees
                    // some of them or none. What is guaranteed is the order and the end: every value seen
                    // is one of the sequence, in sequence order, and the held value is the recovery. The
                    // exact three-value sequence is asserted where nothing is conflated (`HostContractTest`).
                    val sequence = listOf(300uL, 6000uL, 400uL)
                    val seen = health.map { it.sinceHeardMs }
                    assertEquals("in order, nothing repeated or invented", seen, sequence.filter { it in seen })
                    assertEquals(
                        "a stale sample that was seen reads as a stale label",
                        if (6000uL in seen) listOf("Last heard 6 s ago") else emptyList(),
                        health.mapNotNull(::linkStaleLabel),
                    )
                    assertEquals("Last heard 6 s ago", linkStaleLabel(LinkHealth(6000uL)))
                    assertEquals(400uL, mosh.linkHealth.value?.sinceHeardMs)
                    assertNull(linkStaleLabel(mosh.linkHealth.value))

                    // `frameReady` replays its last signal, so a stale one answers `first()` at once: wait
                    // for the frame itself instead, polling the mailbox until the roam shows in row 2.
                    val handle = mosh.handle.value!!
                    val roamed = suspend {
                        withTimeout(5000) {
                            var row: String? = null
                            while (row != "roams 1") {
                                handle.takeFrame()?.changedRows?.find { it.index.toInt() == 2 }
                                    ?.let { row = it.cells.joinToString("") { cell -> cell.text }.trimEnd() }
                                if (row != "roams 1") delay(5)
                            }
                            row
                        }
                    }
                    networkChanged() // What the service's debounced callback calls.
                    assertEquals("roams 1", roamed())

                    holder.disconnect(host.id)
                    withTimeout(5000) { active.state.first { it is HostState.Closed } }
                } finally { key.privateKey.fill(0); holder.release(host.id, closeTerminals = true) }
            }
        }
    }

    @Test
    fun aMoshServerIsRecordedForTheSessionAndOrphansAreStoppedOverTheNewConnectionThroughTheRealFfi() = runBlocking<Unit> {
        Executors.newSingleThreadExecutor().asCoroutineDispatcher().use { main ->
            withContext(main) {
                val prefs = MemoryPrefStore()
                val host = testHost(addresses = listOf(HostEndpoint("probe.invalid", 22)))
                // What an earlier process left: a server the probe host can stop, and one it cannot (13).
                MoshServerLedger(prefs).apply { record(host, 999u); record(host, 13u) }
                val holder = HostConnections(probe, Store(), main, moshServers = MoshServerLedger(prefs))
                val key = generateEd25519Key("probe")
                try {
                    val active = connectedProbe(holder, host, key)
                    // The orphan that was stopped is forgotten; the one whose stop failed stays for next time.
                    withTimeout(5000) { while (MoshServerLedger(prefs).pids(host).contains(999u)) delay(5) }
                    assertEquals(listOf(13u), MoshServerLedger(prefs).pids(host))

                    // A mosh session records the pid its server reported (the probe's is 4242) once connected,
                    // the background one too, when it is swapped in...
                    val mosh = holder.openTerminal(active, TerminalTarget.Tmux("work"))
                    withTimeout(5000) { mosh.transport.first { it == TerminalTransport.MOSH } }
                    withTimeout(5000) { mosh.state.first { it == SessionState.Connected } }
                    assertEquals(4242u, mosh.handle.value!!.serverPid())
                    assertEquals(listOf(13u, 4242u), MoshServerLedger(prefs).pids(host))

                    // ...and forgets it when the user ends the session.
                    holder.disconnectTerminal(mosh)
                    withTimeout(5000) { mosh.state.first { it is SessionState.Closed } }
                    assertEquals(listOf(13u), MoshServerLedger(prefs).pids(host))

                    // The connection itself answers the new FFI call: Ok for a stoppable pid, an error for the probe's 13.
                    active.ready.await().stopMoshServer(1234u)
                    assertThrows(HostException::class.java) { runBlocking { active.ready.await().stopMoshServer(13u) } }
                    assertThrows(HostException.InvalidName::class.java) { runBlocking { active.ready.await().stopMoshServer(0u) } }
                    holder.disconnect(host.id)
                    withTimeout(5000) { active.state.first { it is HostState.Closed } }
                } finally { key.privateKey.fill(0); holder.release(host.id, closeTerminals = true) }
            }
        }
    }

    @Test
    fun aHostPreferringSshGetsSshAgainstTheProbeEvenThoughMoshServerExists() = runBlocking<Unit> {
        Executors.newSingleThreadExecutor().asCoroutineDispatcher().use { main ->
            withContext(main) {
                val store = Store()
                val holder = HostConnections(probe, store, main)
                val key = generateEd25519Key("probe")
                val host = testHost(addresses = listOf(HostEndpoint("probe.invalid", 22)), transport = TransportPref.SSH)
                try {
                    val active = connectedProbe(holder, host, key)
                    val shell = holder.openTerminal(active, TerminalTarget.Shell)
                    assertEquals(TerminalTransport.SSH, shell.transport.value)
                    withTimeout(5000) { shell.state.first { it == SessionState.Connected } }
                    assertNull(shell.linkHealth.value)
                } finally { key.privateKey.fill(0); holder.release(host.id, closeTerminals = true) }
            }
        }
    }
}
