package io.github.code_akram.or2.connection

import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.HostEndpoint
import io.github.code_akram.or2.data.TrustStore
import io.github.code_akram.or2.ffi.AgentStatus
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.HerdrState
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.PublicKeyInfo
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalFrame
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.ffi.contractProbeHost
import io.github.code_akram.or2.ffi.generateEd25519Key
import io.github.code_akram.or2.inbox.INBOX_STATUS_ORDER
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

                    // The capability probe runs once connected; the running default session is watched.
                    val caps = withTimeout(5000) { active.capabilities.first { it != null } }!!
                    assertEquals("/usr/bin/tmux", caps.tmux)
                    assertEquals(listOf("default", "or2-probe"), caps.herdrSessions.map { it.name })
                    assertEquals(listOf<String?>(null), active.watches.value.map { it.session }) // Default is unnamed.
                    assertEquals(listOf("main", "build"), holder.listTmuxSessions(active).map { it.name })

                    // Agents reach the inbox, most urgent status first.
                    val inbox = withTimeout(5000) { holder.inbox(flowOf(listOf(host))).first { it.agentCount == 3 } }
                    val order = inbox.groups.map { it.status }
                    assertEquals(order.sortedBy { INBOX_STATUS_ORDER.indexOf(it) }, order)
                    assertTrue(AgentStatus.IDLE in order)
                    assertEquals(host.label, inbox.groups.first().items.first().hostLabel)
                    assertEquals(inbox.hosts.single().agentCount, 3)
                    val pane = inbox.groups.first().items.first()
                    assertNull(pane.session) // Default session: opened without a name.
                    withTimeout(5000) { active.watches.value.single().state.first { it is HerdrState.Live } }

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
                    assertEquals(HerdrState.Closed, withTimeout(5000) { active.watches.value.single().state.first { it == HerdrState.Closed } })
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
}
