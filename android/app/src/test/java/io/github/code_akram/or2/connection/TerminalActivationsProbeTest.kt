package io.github.code_akram.or2.connection

import io.github.code_akram.or2.app.Activation
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.HostEndpoint
import io.github.code_akram.or2.data.TrustStore
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.PublicKeyInfo
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.ffi.contractProbeHost
import io.github.code_akram.or2.ffi.generateEd25519Key
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import org.junit.Assert.*
import org.junit.Test
import java.util.concurrent.Executors

/**
 * Pane focus end to end over the real FFI: the activation layer on the holder against
 * `contract_probe_host`, which answers `focus_herdr_pane` deterministically (`w1:p1`, `w1:p2` and
 * `w2:p1` succeed, every other id is `PaneNotFound`).
 */
class TerminalActivationsProbeTest {
    private val probe = HostConnector { request, listener -> NativeHostPort(contractProbeHost(request, listener)) }

    private class Store : TrustStore {
        var lines = emptyList<String>()
        override suspend fun trustedKeys(hostId: Long) = lines
        override suspend fun replaceTrust(host: Host, presented: PublicKeyInfo) { lines = listOf(presented.openssh) }
    }

    @Test
    fun focusSucceedsForTheProbePanesAndAGonePaneIsPaneNotFoundWithoutOpeningATerminal() = runBlocking {
        Executors.newSingleThreadExecutor().asCoroutineDispatcher().use { main ->
            withContext(main) {
                val holder = HostConnections(probe, Store(), main)
                val key = generateEd25519Key("probe")
                val host = testHost(addresses = listOf(HostEndpoint("probe.invalid", 22)))
                try {
                    holder.connect(host, key.privateKey.copyOf())
                    val active = holder.host(host.id)!!
                    val prompt = withTimeout(5000) { active.state.first { it is HostState.AwaitingHostKeyDecision } }
                        as HostState.AwaitingHostKeyDecision
                    holder.approve(active, prompt)
                    withTimeout(5000) { active.state.first { it is HostState.Connected } }
                    val activations = holder.activations

                    val a = activations.openAgent(host.id, host.label, null, "w1:p1") as Activation.Ready
                    // B is in A's session: A's terminal is that session's one terminal, shown after B was focused.
                    val b = activations.openAgent(host.id, host.label, null, "w1:p2") as Activation.Ready
                    assertSame(a.terminal, b.terminal)
                    val other = activations.openAgent(host.id, host.label, "or2-probe", "w2:p1") as Activation.Ready
                    assertEquals(2, holder.terminals.value.size)
                    for (terminal in listOf(a, other)) withTimeout(5000) { terminal.terminal.state.first { it == SessionState.Connected } }

                    // A to B to A: the same terminal, after A's pane was focused again.
                    val again = activations.openAgent(host.id, host.label, null, "w1:p1") as Activation.Ready
                    assertSame(a.terminal, again.terminal)
                    assertEquals(2, holder.terminals.value.size)
                    assertEquals(Activation.Ready(a.terminal), activations.reuse(a.terminal))
                    assertEquals(Activation.Ready(other.terminal), activations.reuse(other.terminal))

                    // Any other pane is gone: the explicit message, and no terminal is opened for it.
                    for (gone in listOf("w9:p9", "w1:p3", "w2:p2")) {
                        val failed = activations.openAgent(host.id, host.label, null, gone) as Activation.Failed
                        assertEquals("That agent's pane no longer exists in herdr. Refresh the inbox.", failed.message)
                    }
                    assertEquals(2, holder.terminals.value.size)

                    // A terminal that is open for a pane that has since gone is not shown again either.
                    val stale = holder.openTerminal(active, TerminalTarget.Herdr(null, "w9:p9"))
                    assertTrue(activations.reuse(stale) is Activation.Failed)

                    // Invalid names are an error message, not an activation.
                    val invalid = activations.openAgent(host.id, host.label, "bad name", "w1:p1") as Activation.Failed
                    assertEquals("That name is not valid here. Use letters, digits, dashes and underscores.", invalid.message)
                } finally { key.privateKey.fill(0); holder.release(host.id, closeTerminals = true) }
            }
        }
    }
}
