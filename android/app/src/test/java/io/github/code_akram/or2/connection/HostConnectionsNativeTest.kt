package io.github.code_akram.or2.connection

import io.github.code_akram.or2.OpenSshFixture
import io.github.code_akram.or2.assumeMosh
import io.github.code_akram.or2.assumeSshd
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.HostEndpoint
import io.github.code_akram.or2.data.HostRecord
import io.github.code_akram.or2.data.TransportPref
import io.github.code_akram.or2.data.TrustStore
import io.github.code_akram.or2.ffi.CellWidth
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.HostConnectException
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.PublicKeyInfo
import io.github.code_akram.or2.ffi.SessionFailure
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalFrame
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.ffi.TerminalTransport
import io.github.code_akram.or2.ffi.generateEd25519Key
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import org.junit.Assert.*
import org.junit.Test
import java.net.InetAddress
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
        assumeSshd()
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

    // --- a mosh terminal across an SSH reconnect (real FFI, loopback sshd, local mosh-server) ---------

    /** The screen of one terminal, kept up to date from its frames. */
    private class Screen {
        private val rows = mutableMapOf<Int, String>()

        fun absorb(frame: TerminalFrame) {
            if (frame.full) rows.clear()
            frame.changedRows.forEach { row ->
                rows[row.index.toInt()] = row.cells.joinToString("") { cell ->
                    if (cell.width == CellWidth.SPACER_TAIL) "" else cell.text.ifEmpty { " " }
                }
            }
        }

        fun text() = rows.toSortedMap().values.joinToString("\n")
    }

    private suspend fun <T : Any> eventually(what: String, timeoutMs: Long = 20_000, check: () -> T?): T {
        val end = System.nanoTime() + timeoutMs * 1_000_000
        while (true) {
            check()?.let { return it }
            if (System.nanoTime() > end) throw AssertionError("timed out waiting for $what")
            delay(50)
        }
    }

    private suspend fun awaitText(terminal: ActiveTerminal, screen: Screen, expected: String) {
        eventually("'$expected' on the screen of the terminal (screen: ${screen.text().take(300)})") {
            terminal.handle.value!!.takeFrame()?.let(screen::absorb)
            if (expected in screen.text()) true else null
        }
    }

    /**
     * A connected holder over the production connector with one mosh terminal whose SSH
     * connection is then cut, and the host connected again. [block] gets the lost connection, the
     * surviving terminal, its screen and the reconnected one.
     */
    private fun afterAReconnect(block: suspend (holder: HostConnections, lost: ActiveHost, terminal: ActiveTerminal, screen: Screen, fresh: ActiveHost, host: Host) -> Unit) = runBlocking {
        assumeSshd()
        assumeMosh()
        OpenSshFixture().use { fixture ->
            Executors.newSingleThreadExecutor().asCoroutineDispatcher().use { main ->
                withContext(main) {
                    val holder = HostConnections(HostConnector.Native, Store(), main)
                    val client = generateEd25519Key("")
                    fixture.directory.resolve("authorized").toFile().writeText(client.publicKey.openssh + "\n")
                    val host = Host(
                        HostRecord(1, "Fixture", System.getProperty("user.name")!!, "ephemeral", transport = TransportPref.MOSH),
                        listOf(HostEndpoint(InetAddress.getLoopbackAddress().hostAddress!!, fixture.port)),
                    )
                    try {
                        holder.connect(host, client.privateKey.copyOf())
                        val lost = holder.host(host.id)!!
                        val prompt = withTimeout(10_000) { lost.state.first { it is HostState.AwaitingHostKeyDecision } } as HostState.AwaitingHostKeyDecision
                        holder.approve(lost, prompt)
                        withTimeout(10_000) { lost.state.first { it is HostState.Connected } }

                        val terminal = holder.openTerminal(lost, TerminalTarget.Shell)
                        withTimeout(20_000) { terminal.state.first { it == SessionState.Connected } }
                        assertEquals(TerminalTransport.MOSH, terminal.transport.value)
                        val screen = Screen()
                        terminal.handle.value!!.sendText("stty -echo; printf '\\033[2J\\033[H'; printf 'OR2-%s\\n' READY\n")
                        awaitText(terminal, screen, "OR2-READY")

                        // The network takes the SSH connection; the mosh session carries on.
                        fixture.dropConnections()
                        withTimeout(20_000) { lost.state.first { it is HostState.Closed } }
                        assertTrue(lost.wasLost)
                        assertEquals(SessionState.Connected, terminal.state.value)

                        // The user reconnects the host (the chip, a tap): a new SSH connection.
                        holder.connect(host, client.privateKey.copyOf())
                        val fresh = holder.host(host.id)!!
                        assertNotSame(lost, fresh)
                        withTimeout(10_000) { fresh.state.first { it is HostState.Connected } }
                        block(holder, lost, terminal, screen, fresh, host)
                    } finally {
                        client.privateKey.fill(0)
                        holder.release(host.id, closeTerminals = true)
                    }
                }
            }
        }
    }

    @Test
    fun anSshReconnectKeepsTheSurvivingMoshTerminalAndAnExplicitDisconnectLaterClosesIt() = afterAReconnect { holder, lost, terminal, screen, fresh, host ->
        // The original terminal still works: new input reaches it and new output comes back.
        assertEquals(SessionState.Connected, terminal.state.value)
        terminal.handle.value!!.sendText("printf 'AFTER-%s\\n' RECONNECT\n")
        awaitText(terminal, screen, "AFTER-RECONNECT")
        // Replacing the connection did not release (cancel) the old native connection.
        assertEquals(lost.state.value, lost.mutablePort.value!!.state())

        // Disconnecting the host reaches the terminal of the older connection too.
        holder.disconnect(host.id)
        withTimeout(20_000) { terminal.state.first { it is SessionState.Closed } }
        assertEquals(SessionState.Closed(CloseReason.Disconnected), terminal.state.value)
        withTimeout(10_000) { fresh.state.first { it is HostState.Closed } }
        // With its last mosh terminal gone the older connection is released.
        val released = eventually("the older connection to be released") {
            runCatching { lost.mutablePort.value!!.state() }.exceptionOrNull() as? IllegalStateException
        }
        assertNotNull(released)
    }

    @Test
    fun disconnectAllAfterAnSshReconnectClosesTheSurvivingMoshTerminal() = afterAReconnect { holder, _, terminal, screen, fresh, _ ->
        terminal.handle.value!!.sendText("printf 'STILL-%s\\n' HERE\n")
        awaitText(terminal, screen, "STILL-HERE")
        holder.disconnectAll()
        withTimeout(20_000) { terminal.state.first { it is SessionState.Closed } }
        assertEquals(SessionState.Closed(CloseReason.Disconnected), terminal.state.value)
        withTimeout(10_000) { fresh.state.first { it is HostState.Closed } }
    }
}
