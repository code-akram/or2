package io.github.code_akram.or2.session

import io.github.code_akram.or2.OpenSshFixture
import io.github.code_akram.or2.data.HostRecord
import io.github.code_akram.or2.data.TrustStore
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.ConnectException
import io.github.code_akram.or2.ffi.PublicKeyInfo
import io.github.code_akram.or2.ffi.SessionFailure
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.connect
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

/** Exercises the production connector; only a disposable loopback sshd is contacted. */
class SessionHolderNativeTest {
    private class Store : TrustStore {
        var lines = emptyList<String>()
        var replacements = 0
        override suspend fun trustedKeys(hostId: Long) = lines
        override suspend fun replaceTrust(host: HostRecord, presented: PublicKeyInfo) {
            lines = listOf(presented.openssh)
            replacements++
        }
    }

    @Test
    fun productionValidationFailureWipesWithoutLeavingAnActiveSession() = runBlocking {
        Executors.newSingleThreadExecutor().asCoroutineDispatcher().use { main ->
            withContext(main) {
                val holder = SessionHolder(SessionConnector(::connect), Store(), main)
                val material = generateEd25519Key("")
                val host = HostRecord(1, "Fixture", "", 22, "fixture", "ephemeral")
                try {
                    try { holder.connect(host, material.privateKey); fail("Expected invalid host") }
                    catch (_: ConnectException.InvalidHost) { }
                    assertTrue(material.privateKey.all { it == 0.toByte() })
                    assertNull(holder.active.value)
                } finally { material.privateKey.fill(0); holder.dismiss() }
            }
        }
    }

    @Test
    fun productionFirstUsePersistTrustedReconnectChangedRejectAndRetainedClose() = runBlocking {
        assumeTrue("sshd is not installed", Files.isExecutable(Path.of("/usr/bin/sshd")))
        OpenSshFixture().use { fixture ->
            Executors.newSingleThreadExecutor().asCoroutineDispatcher().use { main ->
                withContext(main) {
                    val store = Store()
                    val holder = SessionHolder(SessionConnector(::connect), store, main)
                    val client = generateEd25519Key("")
                    fixture.directory.resolve("authorized").toFile().writeText(client.publicKey.openssh + "\n")
                    val host = HostRecord(1, "Fixture", InetAddress.getLoopbackAddress().hostAddress!!,
                        fixture.port, System.getProperty("user.name")!!, "ephemeral")
                    val bytes = client.privateKey.copyOf()
                    try {
                        holder.connect(host, bytes)
                        assertTrue(bytes.all { it == 0.toByte() })
                        val first = holder.active.value!!
                        val prompt = withTimeout(5000) { first.state.first { it is SessionState.AwaitingHostKeyDecision } }
                            as SessionState.AwaitingHostKeyDecision
                        assertEquals(fixture.fingerprint, prompt.presented.fingerprint)
                        assertTrue(prompt.previouslyTrusted.isEmpty())
                        holder.approve(first, prompt)
                        withTimeout(5000) { first.state.first { it == SessionState.Connected } }
                        assertEquals(listOf(prompt.presented.openssh), store.lines)
                        assertEquals(1, store.replacements)
                        holder.disconnect()
                        withTimeout(5000) { first.state.first { it is SessionState.Closed } }
                        assertEquals(SessionState.Closed(CloseReason.Disconnected), first.state.value)
                        assertSame(first, holder.active.value)
                        assertEquals(first.state.value, first.handle.value!!.state()) // Still usable, not destroyed.
                        holder.dismiss()
                        assertThrows(IllegalStateException::class.java) { first.handle.value!!.state() }

                        holder.connect(host, client.privateKey.copyOf())
                        val trusted = holder.active.value!!
                        withTimeout(5000) { trusted.state.first { it == SessionState.Connected } }
                        assertEquals(1, store.replacements) // No new prompt or persistence for matching key.

                        val unrelated = generateEd25519Key("")
                        try { store.lines = listOf(unrelated.publicKey.openssh) } finally { unrelated.privateKey.fill(0) }
                        holder.connect(host, client.privateKey.copyOf())
                        val changed = holder.active.value!!
                        val changedPrompt = withTimeout(5000) { changed.state.first { it is SessionState.AwaitingHostKeyDecision } }
                            as SessionState.AwaitingHostKeyDecision
                        assertEquals(fixture.fingerprint, changedPrompt.presented.fingerprint)
                        assertEquals(store.lines.single(), changedPrompt.previouslyTrusted.single().openssh)
                        holder.reject(changed)
                        val closed = withTimeout(5000) { changed.state.first { it is SessionState.Closed } }
                        assertEquals(SessionState.Closed(CloseReason.Failed(SessionFailure.HostKeyRejected)), closed)
                        assertEquals(1, store.replacements) // Reject never overwrites old trust.
                    } finally { bytes.fill(0); client.privateKey.fill(0); holder.dismiss() }
                }
            }
        }
    }
}
