package io.github.code_akram.or2.connection

import io.github.code_akram.or2.OpenSshFixture
import io.github.code_akram.or2.assumeConnectHostIsReal
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.HostEndpoint
import io.github.code_akram.or2.data.HostRecord
import io.github.code_akram.or2.data.TrustStore
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.HostConnectException
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.PublicKeyInfo
import io.github.code_akram.or2.ffi.SessionFailure
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.ffi.generateEd25519Key
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Test
import java.net.InetAddress
import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.Executors

/** Exercises the production connector (`connect_host`); only a disposable loopback sshd is contacted. */
class HostConnectionsNativeTest {
    private class Store : TrustStore {
        var lines = emptyList<String>()
        var replacements = 0
        override suspend fun trustedKeys(hostId: Long) = lines
        override suspend fun replaceTrust(host: Host, presented: PublicKeyInfo) {
            lines = listOf(presented.openssh)
            replacements++
        }
    }

    @Test
    fun productionValidationFailureWipesWithoutLeavingAConnection() = runBlocking {
        Executors.newSingleThreadExecutor().asCoroutineDispatcher().use { main ->
            withContext(main) {
                val holder = HostConnections(HostConnector.Native, Store(), main)
                val material = generateEd25519Key("")
                val host = Host(HostRecord(1, "Fixture", "fixture", "ephemeral"), listOf(HostEndpoint("", 22)))
                try {
                    try { holder.connect(host, material.privateKey); fail("Expected an invalid address") }
                    catch (error: HostConnectException.InvalidAddress) { assertEquals(0u, error.index) }
                    assertTrue(material.privateKey.all { it == 0.toByte() })
                    assertTrue(holder.hosts.value.isEmpty())
                } finally { material.privateKey.fill(0) }
            }
        }
    }

    @Test
    fun productionFirstUsePersistTrustedReconnectChangedRejectTerminalAndRetainedClose() = runBlocking {
        assumeTrue("sshd is not installed", Files.isExecutable(Path.of("/usr/bin/sshd")))
        assumeConnectHostIsReal()
        OpenSshFixture().use { fixture ->
            Executors.newSingleThreadExecutor().asCoroutineDispatcher().use { main ->
                withContext(main) {
                    val store = Store()
                    val holder = HostConnections(HostConnector.Native, store, main)
                    val client = generateEd25519Key("")
                    fixture.directory.resolve("authorized").toFile().writeText(client.publicKey.openssh + "\n")
                    val host = Host(
                        HostRecord(1, "Fixture", System.getProperty("user.name")!!, "ephemeral"),
                        listOf(HostEndpoint(InetAddress.getLoopbackAddress().hostAddress!!, fixture.port)),
                    )
                    val bytes = client.privateKey.copyOf()
                    try {
                        holder.connect(host, bytes)
                        assertTrue(bytes.all { it == 0.toByte() })
                        val first = holder.host(host.id)!!
                        val prompt = withTimeout(5000) { first.state.first { it is HostState.AwaitingHostKeyDecision } }
                            as HostState.AwaitingHostKeyDecision
                        assertEquals(fixture.fingerprint, prompt.presented.fingerprint)
                        assertTrue(prompt.previouslyTrusted.isEmpty())
                        holder.approve(first, prompt)
                        withTimeout(5000) { first.state.first { it is HostState.Connected } }
                        assertEquals(listOf(prompt.presented.openssh), store.lines)
                        assertEquals(1, store.replacements)

                        val shell = holder.openTerminal(first, TerminalTarget.Shell)
                        withTimeout(5000) { shell.state.first { it == SessionState.Connected } }

                        holder.disconnect(host.id)
                        withTimeout(5000) { first.state.first { it is HostState.Closed } }
                        assertEquals(HostState.Closed(CloseReason.Disconnected), first.state.value)
                        withTimeout(5000) { shell.state.first { it is SessionState.Closed } }
                        assertSame(first, holder.host(host.id))
                        assertEquals(first.state.value, first.mutablePort.value!!.state()) // Still usable, not destroyed.
                        holder.dismissHost(host.id)
                        assertThrows(IllegalStateException::class.java) { first.mutablePort.value!!.state() }
                        holder.dismissTerminal(shell)

                        holder.connect(host, client.privateKey.copyOf())
                        val trusted = holder.host(host.id)!!
                        withTimeout(5000) { trusted.state.first { it is HostState.Connected } }
                        assertEquals(1, store.replacements) // No new prompt or persistence for a matching key.
                        holder.disconnect(host.id)
                        withTimeout(5000) { trusted.state.first { it is HostState.Closed } }

                        val unrelated = generateEd25519Key("")
                        try { store.lines = listOf(unrelated.publicKey.openssh) } finally { unrelated.privateKey.fill(0) }
                        holder.connect(host, client.privateKey.copyOf())
                        val changed = holder.host(host.id)!!
                        val changedPrompt = withTimeout(5000) { changed.state.first { it is HostState.AwaitingHostKeyDecision } }
                            as HostState.AwaitingHostKeyDecision
                        assertEquals(fixture.fingerprint, changedPrompt.presented.fingerprint)
                        assertEquals(store.lines.single(), changedPrompt.previouslyTrusted.single().openssh)
                        holder.reject(changed)
                        val closed = withTimeout(5000) { changed.state.first { it is HostState.Closed } }
                        assertEquals(HostState.Closed(CloseReason.Failed(SessionFailure.HostKeyRejected)), closed)
                        assertEquals(1, store.replacements) // Reject never overwrites old trust.
                    } finally { bytes.fill(0); client.privateKey.fill(0); holder.release(host.id, closeTerminals = true) }
                }
            }
        }
    }
}
