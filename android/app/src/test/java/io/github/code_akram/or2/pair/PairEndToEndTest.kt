package io.github.code_akram.or2.pair

import io.github.code_akram.or2.OpenSshFixture
import io.github.code_akram.or2.assumeSshd
import io.github.code_akram.or2.connection.HostConnections
import io.github.code_akram.or2.connection.HostConnector
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.HostEndpoint
import io.github.code_akram.or2.data.KeyRecord
import io.github.code_akram.or2.data.TrustStore
import io.github.code_akram.or2.ffi.ClientKeyMaterial
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.PublicKeyInfo
import io.github.code_akram.or2.ffi.generateEd25519Key
import io.github.code_akram.or2.ffi.pairNewCode
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
import java.nio.file.Path
import java.util.concurrent.Executors
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit

/** `<algorithm> <base64>` of a key line, without any comment. */
private fun plain(line: String) = line.split(' ').take(2).joinToString(" ")

/**
 * Easy pair v2 end to end on loopback, as the contract's "Tests" section describes: the real `or2-pair`
 * (built with `test-support` as `or2-pair-testhost`) in a temporary home, behind a disposable sshd whose
 * `AuthorizedKeysFile` the CLI writes; the real native code, parser and `pair_enroll` over `DirectTcp`; the
 * pairing flow; and then a real `connect_host` to the same sshd with the host key and the key the pairing
 * installed. Nothing of the user's own `~/.ssh`, sshd, tmux or herdr is involved.
 *
 * **What the test relies on in `or2-pair-testhost`** (the contract's `or2-pair-testhost` paragraph):
 * - `--name`, `--user`, `--ssh-port` and `--address` work as for `or2-pair`, and the forced command it installs
 *   runs the internal `enroll` subcommand of the same binary;
 * - the environment names `OR2_PAIR_TEST_HOME`, `OR2_PAIR_TEST_USER` and `OR2_PAIR_TEST_AUTHORIZED_KEYS` of the
 *   contract, and `OR2_PAIR_TEST_ETC_SSH` (a directory standing in for `/etc/ssh`, where it finds
 *   `ssh_host_ed25519_key.pub`; recorded in the contract under `or2-pair-testhost`), all of them also visible
 *   to the forced command (`enroll`), which sshd starts with the sessions' environment (the fixture's `SetEnv`);
 * - `K` is read from standard input, one line, with no terminal;
 * - standard output carries the QR's text on a line of its own (it starts with `or2-pair:2?`) and, once the
 *   phone is enrolled, the line `Paired "<device>" (SHA256:…) as <user>.` of the contract's sample output.
 * Like the other sshd suites it skips without `/usr/bin/sshd`, and fails instead when `OR2_REQUIRE_SSHD` is set.
 */
class PairEndToEndTest {
    /** The CLI host: a child process whose standard input takes `K` and whose standard output carries the QR. */
    private class CliHost(val home: File, etcSsh: File, authorized: Path, user: String, sshPort: Int) : AutoCloseable {
        private val lines = LinkedBlockingQueue<String>()
        private val process: Process

        init {
            process = ProcessBuilder(
                testHostPath(), "--name", "Test Host", "--user", user, "--ssh-port", sshPort.toString(), "--address", "127.0.0.1",
            ).apply {
                environment().putAll(
                    mapOf(
                        "OR2_PAIR_TEST_HOME" to home.path, "OR2_PAIR_TEST_USER" to user,
                        "OR2_PAIR_TEST_AUTHORIZED_KEYS" to authorized.toString(), "OR2_PAIR_TEST_ETC_SSH" to etcSsh.path,
                    ),
                )
            }.redirectError(File(home.parentFile, "cli.log")).start()
            Thread {
                process.inputStream.bufferedReader().forEachLine { lines.put(it) }
            }.apply { isDaemon = true }.start()
        }

        /** Types [code] at the prompt (the testhost reads it from standard input). */
        fun type(code: String) {
            process.outputStream.apply {
                write("$code\n".toByteArray())
                flush()
            }
        }

        private fun line(prefix: String): String {
            val end = System.nanoTime() + TimeUnit.SECONDS.toNanos(20)
            while (true) {
                val next = lines.poll(100, TimeUnit.MILLISECONDS)
                if (next != null && next.startsWith(prefix)) return next.trim()
                check(System.nanoTime() < end) { "no '$prefix' line from or2-pair-testhost" }
            }
        }

        /** The QR's text, once the CLI has accepted the code and added the bootstrap key. */
        fun payload() = line("or2-pair:2?")

        /** The CLI's last line, once the phone is enrolled. */
        fun result() = line("Paired \"")

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

    /** The disposable sshd, the CLI's temporary home and a stand-in `/etc/ssh` with the sshd's host key. */
    private class World(val fixture: OpenSshFixture, val home: File, val etcSsh: File, val user: String) : AutoCloseable {
        val authorized: Path get() = fixture.directory.resolve("authorized")
        override fun close() = fixture.close()
    }

    private fun world(): World {
        val user = System.getProperty("user.name")!!
        val fixture = OpenSshFixture { directory ->
            // The forced command runs under sshd: it needs the CLI's test environment as well.
            listOf(
                "OR2_PAIR_TEST_HOME=${directory.resolve("clihome")}", "OR2_PAIR_TEST_USER=$user",
                "OR2_PAIR_TEST_AUTHORIZED_KEYS=${directory.resolve("authorized")}", "OR2_PAIR_TEST_ETC_SSH=${directory.resolve("etc")}",
            )
        }
        val etc = fixture.directory.resolve("etc").toFile().apply { mkdirs() }
        File(etc, "ssh_host_ed25519_key.pub").writeText(fixture.hostPublicKey + "\n")
        return World(fixture, fixture.directory.resolve("clihome").toFile().apply { mkdirs() }, etc, user)
    }

    @Test
    fun pairingThenConnectingNeedsNoFirstUsePromptAndAuthenticatesWithThePairedKey() = runBlocking<Unit> {
        assumeSshd()
        world().use { world ->
            val fixture = world.fixture
            CliHost(world.home, world.etcSsh, world.authorized, world.user, fixture.port).use { cli ->
                val trust = Trust()
                val store = Store(trust)
                val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
                var material: ClientKeyMaterial? = null
                try {
                    val flow = PairFlow(NativePair, store, scope)
                    // The code on the phone's screen is what the person types at the host's prompt.
                    cli.type((flow.state.value as PairState.Scanning).code.text)
                    val qr = cli.payload()
                    assertTrue(flow.onCode(qr, emptyList()))
                    val review = (flow.state.value as PairState.Review).review
                    assertTrue(review.enrolls)
                    assertEquals(world.user, review.username)
                    assertEquals(fixture.fingerprint, review.offer.hostKey.fingerprint)
                    assertEquals(fixture.port, review.offer.port.toInt())
                    assertEquals("127.0.0.1", review.offer.addresses.first().host)
                    assertEquals(KeyChoice.New, review.choice)

                    flow.submit(emptyList(), "Test Phone") { label, comment ->
                        val made = generateEd25519Key(comment)
                        material = made
                        KeyRecord("paired", label, made.publicKey.algorithm, made.publicKey.openssh, made.publicKey.fingerprint, comment, ByteArray(0), ByteArray(0))
                    }
                    val outcome = withTimeout(30_000) {
                        flow.state.first { it is PairState.Paired || (it is PairState.Review && it.review.error != null) || (it is PairState.Scanning && it.error != null) }
                    }
                    assertTrue("pairing failed: $outcome", outcome is PairState.Paired)
                    cli.result()

                    // What the CLI left in the sshd's AuthorizedKeysFile: the phone's key, with its options and comment, and
                    // no trace of the bootstrap key.
                    val authorized = world.authorized.toFile().readText()
                    val key = material!!.publicKey
                    assertEquals(
                        "no-agent-forwarding,no-X11-forwarding ${plain(key.openssh)} or2-Test-Phone-${java.time.LocalDate.now(java.time.ZoneOffset.UTC)}\n",
                        authorized,
                    )
                    assertFalse(authorized.contains("or2-pair-bootstrap"))
                    // The run's state is gone with it.
                    assertEquals(emptyList<String>(), File(world.home, ".ssh/or2-pair").list()?.toList().orEmpty())

                    // The host was saved with the host key trusted, from the code, before any connection.
                    val host = (outcome as PairState.Paired).host
                    assertEquals("Test Host", host.label)
                    assertEquals(listOf(plain(fixture.hostPublicKey)), trust.lines.map(::plain))

                    Executors.newSingleThreadExecutor().asCoroutineDispatcher().use { main ->
                        withContext(main) {
                            val holder = HostConnections(HostConnector.Native, trust, main)
                            val connectable = host.copy(
                                record = host.record.copy(username = world.user),
                                addresses = listOf(HostEndpoint(InetAddress.getLoopbackAddress().hostAddress!!, fixture.port)),
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
        }
    }

    @Test
    fun aDifferentCodeTypedOnTheHostIsRefusedWithANewCodeAndNothingSavedOrAuthorized() = runBlocking<Unit> {
        assumeSshd()
        world().use { world ->
            CliHost(world.home, world.etcSsh, world.authorized, world.user, world.fixture.port).use { cli ->
                val trust = Trust()
                val store = Store(trust)
                val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
                try {
                    val flow = PairFlow(NativePair, store, scope)
                    val onThePhone = (flow.state.value as PairState.Scanning).code.text
                    // A typo that passes the check character is a different code: the bootstrap keys differ.
                    val typed = pairNewCode().display()
                    cli.type(typed)
                    flow.onCode(cli.payload(), emptyList())
                    flow.submit(emptyList(), "Test Phone") { _, comment ->
                        val made = generateEd25519Key(comment)
                        made.privateKey.fill(0)
                        KeyRecord("k", "Key", made.publicKey.algorithm, made.publicKey.openssh, made.publicKey.fingerprint, comment, ByteArray(0), ByteArray(0))
                    }
                    val outcome = withTimeout(30_000) {
                        flow.state.first { it is PairState.Paired || (it is PairState.Review && it.review.error != null) || (it is PairState.Scanning && it.error != null) }
                    }
                    val scanning = outcome as? PairState.Scanning ?: error("expected a refusal that reached the host, got $outcome")
                    assertTrue(scanning.error!!, scanning.error!!.contains("didn't accept the pairing key"))
                    // The host saw the code: it is spent, and the screen shows a new one.
                    assertTrue(onThePhone != scanning.code.text)
                    assertNull(store.saved)
                    assertTrue(trust.lines.isEmpty())
                    // Only the bootstrap entry is on the host, and no phone key.
                    assertFalse(world.authorized.toFile().readText().contains("or2-Test-Phone"))
                } finally {
                    scope.cancel()
                }
            }
        }
    }
}

private fun testHostPath(): String {
    val path = System.getProperty("or2.pair.testhost")
    check(path != null && File(path).canExecute()) { "or2-pair-testhost was not built ($path)" }
    return path
}
