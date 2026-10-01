package io.github.code_akram.or2.pair

import io.github.code_akram.or2.OpenSshFixture
import io.github.code_akram.or2.assumeSshd
import io.github.code_akram.or2.connection.HostConnections
import io.github.code_akram.or2.connection.HostConnector
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.KeyRecord
import io.github.code_akram.or2.data.TrustStore
import io.github.code_akram.or2.ffi.ClientKeyMaterial
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.PublicKeyInfo
import io.github.code_akram.or2.ffi.generateEd25519Key
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File
import java.net.InetAddress
import java.nio.file.Files
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.Executors

/** `<algorithm> <base64>` of a key line, without any comment. */
private fun plain(line: String) = line.split(' ').take(2).joinToString(" ")

/**
 * Easy pair end to end on loopback: the real `or2-pair` flow (built with its test-only auto-confirming host,
 * `or2-pair-testhost`) in a temporary home, the real native parser and exchange over `DirectTcp`, the pairing flow,
 * and then a real `connect_host` to a disposable sshd using the host key and the key the pairing established. Nothing
 * of the user's own `~/.ssh`, sshd, tmux or herdr is involved.
 */
class PairEndToEndTest {
    /** The CLI host: a child process whose stdout carries the pairing code. */
    private class CliHost(val home: File, etc: File, sshPort: Int, answer: String = "yes") : AutoCloseable {
        private val lines = LinkedBlockingQueue<String>()
        private val process: Process

        init {
            val path = System.getProperty("or2.pair.testhost")
            check(path != null && File(path).canExecute()) { "or2-pair-testhost was not built ($path)" }
            process = ProcessBuilder(
                path, "--home", home.path, "--etc", etc.path, "--ssh-port", sshPort.toString(),
                "--address", "127.0.0.1", "--answer", answer, "--window-secs", "60",
            ).redirectError(File(home.parentFile, "cli.log")).start()
            Thread {
                process.inputStream.bufferedReader().forEachLine { lines.put(it) }
            }.apply { isDaemon = true }.start()
        }

        private fun line(prefix: String): String {
            val end = System.nanoTime() + TimeUnit.SECONDS.toNanos(20)
            while (true) {
                val next = lines.poll(100, TimeUnit.MILLISECONDS)
                if (next != null && next.startsWith(prefix)) return next.removePrefix(prefix).trim()
                check(System.nanoTime() < end) { "no '$prefix' line from or2-pair-testhost" }
            }
        }

        /** The pairing code, once the listener is up. */
        fun payload() = line("PAYLOAD ")

        /** How the CLI ended (`Paired`, `Declined`, ...). */
        fun result() = line("RESULT ")

        override fun close() {
            process.destroyForcibly()
            process.waitFor(5, TimeUnit.SECONDS)
        }
    }

    private class Trust : TrustStore {
        var lines = emptyList<String>()
        override suspend fun trustedKeys(hostId: Long) = lines
        override suspend fun replaceTrust(host: Host, presented: PublicKeyInfo) = error("pairing never goes through the prompt path")
    }

    private class Store(private val trust: Trust) : PairStore {
        var saved: Host? = null
        override suspend fun saveTrustedHost(host: Host, hostKey: PublicKeyInfo): Host {
            trust.lines = listOf(hostKey.openssh)
            return host.copy(record = host.record.copy(id = 1)).also { saved = it }
        }
    }

    private fun world(fixture: OpenSshFixture): Pair<File, File> {
        val work = Files.createTempDirectory("or2-pair-e2e-").toFile()
        val etc = File(work, "etc").apply { mkdirs() }
        File(etc, "ssh_host_ed25519_key.pub").writeText(fixture.hostPublicKey + "\n")
        return File(work, "home").apply { mkdirs() } to etc
    }

    @Test
    fun pairingThenConnectingNeedsNoFirstUsePromptAndAuthenticatesWithThePairedKey() = runBlocking<Unit> {
        assumeSshd()
        OpenSshFixture().use { fixture ->
            val (home, etc) = world(fixture)
            try {
                CliHost(home, etc, fixture.port).use { cli ->
                    val code = cli.payload()
                    val trust = Trust()
                    val store = Store(trust)
                    val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
                    var material: ClientKeyMaterial? = null
                    try {
                        val flow = PairFlow(NativePair, store, scope)
                        assertTrue(flow.onCode(code, emptyList()))
                        val review = (flow.state.value as PairState.Review).review
                        assertEquals(fixture.fingerprint, review.offer.hostKey.fingerprint)
                        assertEquals(listOf("127.0.0.1" to fixture.port, "testhost.local" to fixture.port), review.offer.addresses.map { it.host to it.port.toInt() })
                        assertEquals(KeyChoice.New, review.choice)

                        flow.submit(emptyList(), "Test Phone") { label, comment ->
                            val made = generateEd25519Key(comment)
                            material = made
                            KeyRecord("paired", label, made.publicKey.algorithm, made.publicKey.openssh, made.publicKey.fingerprint, comment, ByteArray(0), ByteArray(0))
                        }
                        val paired = withTimeout(30_000) { flow.state.first { it is PairState.Paired || (it is PairState.Review && it.review.error != null) } }
                        assertTrue("pairing failed: ${(paired as? PairState.Review)?.review?.error}", paired is PairState.Paired)
                        assertEquals("Paired", cli.result())

                        // What the CLI put in the (temporary) home: the phone's key, with the options and comment.
                        val authorized = File(home, ".ssh/authorized_keys").readText()
                        val key = material!!.publicKey
                        assertEquals(
                            "no-agent-forwarding,no-X11-forwarding ${plain(key.openssh)} or2-Test-Phone-${java.time.LocalDate.now(java.time.ZoneOffset.UTC)}\n",
                            authorized,
                        )
                        assertEquals(1, authorized.lines().count { it.isNotBlank() })

                        // The host was saved with the host key trusted, from the code, before any connection.
                        val host = (paired as PairState.Paired).host
                        assertEquals("Test Host", host.label)
                        assertEquals(listOf(plain(fixture.hostPublicKey)), trust.lines.map(::plain))

                        // sshd reads its own AuthorizedKeysFile: give it what the CLI wrote, as sshd would read ~/.ssh.
                        fixture.directory.resolve("authorized").toFile().writeText(authorized)
                        Executors.newSingleThreadExecutor().asCoroutineDispatcher().use { main ->
                            withContext(main) {
                                val holder = HostConnections(HostConnector.Native, trust, main)
                                val connectable = host.copy(
                                    record = host.record.copy(username = System.getProperty("user.name")!!),
                                    addresses = listOf(io.github.code_akram.or2.data.HostEndpoint(InetAddress.getLoopbackAddress().hostAddress!!, fixture.port)),
                                )
                                val bytes = material!!.privateKey.copyOf()
                                try {
                                    holder.connect(connectable, bytes)
                                    val active = holder.host(connectable.id)!!
                                    // Straight to Connected: the pairing already trusted the host key.
                                    val state = withTimeout(10_000) {
                                        active.state.first { it is HostState.Connected || it is HostState.AwaitingHostKeyDecision || it is HostState.Closed }
                                    }
                                    assertTrue("expected Connected without a prompt, got $state", state is HostState.Connected)
                                    holder.disconnect(connectable.id)
                                    withTimeout(5_000) { active.state.first { it is HostState.Closed } }
                                } finally {
                                    bytes.fill(0)
                                    holder.release(connectable.id, closeTerminals = true)
                                }
                            }
                        }
                    } finally {
                        material?.privateKey?.fill(0)
                        scope.cancel()
                    }
                }
            } finally {
                home.parentFile!!.deleteRecursively()
            }
        }
    }

    @Test
    fun decliningOnTheHostLeavesNothingSavedAndNothingAuthorized() = runBlocking<Unit> {
        assumeSshd()
        OpenSshFixture().use { fixture ->
            val (home, etc) = world(fixture)
            try {
                CliHost(home, etc, fixture.port, answer = "no").use { cli ->
                    val code = cli.payload()
                    val trust = Trust()
                    val store = Store(trust)
                    val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
                    try {
                        val flow = PairFlow(NativePair, store, scope)
                        flow.onCode(code, emptyList())
                        val secret = (flow.state.value as PairState.Review).review.offer.exchange!!.secret
                        flow.submit(emptyList(), "Test Phone") { _, comment ->
                            val made = generateEd25519Key(comment)
                            made.privateKey.fill(0)
                            KeyRecord("k", "Key", made.publicKey.algorithm, made.publicKey.openssh, made.publicKey.fingerprint, comment, ByteArray(0), ByteArray(0))
                        }
                        val outcome = withTimeout(30_000) { flow.state.first { it is PairState.Paired || (it is PairState.Review && it.review.error != null) } }
                        val review = (outcome as PairState.Review).review
                        assertTrue(review.error!!, review.error!!.contains("declined"))
                        assertEquals("Declined", cli.result())
                        assertNull(store.saved)
                        assertTrue(trust.lines.isEmpty())
                        assertFalse(File(home, ".ssh").exists())
                        // The host refused: the code is spent, and the native side wiped its password.
                        assertTrue(secret.isWiped())
                    } finally {
                        scope.cancel()
                    }
                }
            } finally {
                home.parentFile!!.deleteRecursively()
            }
        }
    }
}
