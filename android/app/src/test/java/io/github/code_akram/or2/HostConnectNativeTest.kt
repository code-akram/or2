package io.github.code_akram.or2

import io.github.code_akram.or2.ffi.CellWidth
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.HostAddress
import io.github.code_akram.or2.ffi.HostConnectRequest
import io.github.code_akram.or2.ffi.HostException
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.KeyInput
import io.github.code_akram.or2.ffi.KeyModifiers
import io.github.code_akram.or2.ffi.Session
import io.github.code_akram.or2.ffi.SessionFailure
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalFrame
import io.github.code_akram.or2.ffi.TerminalKey
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.ffi.TerminalTransport
import io.github.code_akram.or2.ffi.connectHost
import io.github.code_akram.or2.ffi.networkChanged
import io.github.code_akram.or2.ffi.generateEd25519Key
import java.net.InetAddress
import java.net.ServerSocket
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The production host connection (`connectHost`) through the real FFI against a disposable
 * loopback sshd (`OpenSshFixture`): address racing, first-use trust, suspend queries, a shell
 * terminal and close ordering. Skipped when `/usr/bin/sshd` is absent (a failure under `OR2_REQUIRE_SSHD`). Only temporary keys and
 * configuration are used; tmux sessions in the fixture would live on a private socket.
 */
class HostConnectNativeTest {
    private val loopback = checkNotNull(InetAddress.getLoopbackAddress().hostAddress)
    private val user = checkNotNull(System.getProperty("user.name"))

    private fun deadPort() = ServerSocket(0, 1, InetAddress.getLoopbackAddress()).use { it.localPort }

    private fun request(
        fixture: OpenSshFixture,
        key: ByteArray,
        trusted: List<String> = emptyList(),
        deadFirst: Boolean = false,
    ): HostConnectRequest {
        val live = HostAddress(loopback, fixture.port.toUShort())
        val addresses = if (deadFirst) listOf(HostAddress(loopback, deadPort().toUShort()), live) else listOf(live)
        return HostConnectRequest(addresses, user, key, trusted)
    }

    @Test
    fun connectHostApprovesTrustQueriesAShellAndClosesTerminalsBeforeTheHost() {
        assumeSshd()
        OpenSshFixture().use { fixture ->
            val key = generateEd25519Key("")
            try {
                fixture.directory.resolve("authorized").toFile().writeText(key.publicKey.openssh + "\n")
                val testThread = Thread.currentThread()
                val timeline = CopyOnWriteArrayList<String>()

                // First use, racing a dead first address: the second one wins.
                val recorder = HostRecorder().also { it.timeline = timeline; it.timelineTag = "host" }
                var trustedLine = ""
                connectHost(request(fixture, key.privateKey.copyOf(), deadFirst = true), recorder).use { host ->
                    val prompt = recorder.await<HostState.AwaitingHostKeyDecision>()
                    assertEquals(fixture.fingerprint, prompt.presented.fingerprint)
                    assertTrue(prompt.previouslyTrusted.isEmpty())
                    trustedLine = prompt.presented.openssh
                    assertEquals(prompt, host.state())
                    assertThrows(HostException.HostKeyMismatch::class.java) {
                        host.approveHostKey(key.publicKey.fingerprint)
                    }
                    host.approveHostKey(prompt.presented.fingerprint)
                    recorder.await<HostState.Authenticating>()
                    assertEquals(HostState.Connected(1u), recorder.await<HostState.Connected>())

                    // Suspend queries across the real FFI.
                    val capabilities = runBlocking { host.capabilities() }
                    assertTrue(capabilities.utf8Locale.lowercase().contains("utf"))
                    capabilities.tmux?.let { assertTrue(it.startsWith("/")) }
                    // The fixture's tmux socket directory is private and empty: no server.
                    if (capabilities.tmux != null) assertTrue(runBlocking { host.listTmuxSessions() }.isEmpty())

                    // A shell terminal on the connection: echo, resize, exit status.
                    val shell = RecordingListener().also { it.timeline = timeline; it.timelineTag = "shell" }
                    val session = host.openTerminal(TerminalTarget.Shell, TerminalTransport.SSH, 80u, 24u, shell)
                    assertEquals(SessionState.Connected, shell.awaitState<SessionState.Connected>())
                    val grid = mutableMapOf<Int, String>()
                    session.sendText("stty -echo; printf '\\033[2J\\033[H'; printf 'OR2-%s\\n' READY\n")
                    awaitText(session, shell, grid, "OR2-READY")
                    session.resize(97u, 31u)
                    session.sendText("printf 'SIZE:'; stty size\n")
                    val resized = awaitText(session, shell, grid, "SIZE:31 97")
                    assertEquals(97.toUShort(), resized.columns)
                    assertEquals(31.toUShort(), resized.rows)
                    session.sendText("printf 'UTF-%s\\n' 'é界😀'\n")
                    awaitText(session, shell, grid, "UTF-é界😀")
                    session.sendText("printf 'KEY-%s\\n' ")
                    session.sendKey(KeyInput(TerminalKey.Character("a"), KeyModifiers(false, false, false, false)))
                    session.sendKey(KeyInput(TerminalKey.Enter, KeyModifiers(false, false, false, false)))
                    awaitText(session, shell, grid, "KEY-a")

                    // A second terminal shares the connection; ending the first leaves it up.
                    val second = RecordingListener().also { it.timeline = timeline; it.timelineTag = "second" }
                    val other = host.openTerminal(TerminalTarget.Shell, TerminalTransport.SSH, 60u, 20u, second)
                    second.awaitState<SessionState.Connected>()
                    session.sendText("exit 17\n")
                    assertEquals(CloseReason.RemoteExited(17u), shell.awaitState<SessionState.Closed>().reason)
                    assertEquals(HostState.Connected(1u), host.state())
                    other.sendText("printf 'LIVE-%s\\n' OK\n")
                    awaitText(other, second, mutableMapOf(), "LIVE-OK")

                    // User disconnect: the remaining terminal closes, then the host.
                    host.disconnect()
                    assertEquals(CloseReason.Disconnected, second.awaitState<SessionState.Closed>().reason)
                    assertEquals(CloseReason.Disconnected, recorder.await<HostState.Closed>().reason)
                    assertThrows(HostException.Closed::class.java) { runBlocking { host.capabilities() } }
                    assertThrows(HostException.Closed::class.java) {
                        host.openTerminal(TerminalTarget.Shell, TerminalTransport.SSH, 80u, 24u, RecordingListener())
                    }
                    recorder.assertQuiet()
                    second.assertNoMoreStates()
                    assertFalse(recorder.overlapped || shell.overlapped || second.overlapped)
                    assertFalse(testThread in recorder.callbackThreads)
                    assertFalse(testThread in shell.callbackThreads)
                }
                assertEquals(
                    "terminals close before the host: $timeline",
                    listOf("second:Closed", "host:Closed"),
                    timeline.filter { it.endsWith(":Closed") }.filter { it != "shell:Closed" },
                )

                // Trusted reconnect: no prompt. Changed key: prompt shows the old one; reject closes.
                val again = HostRecorder()
                connectHost(request(fixture, key.privateKey.copyOf(), listOf(trustedLine)), again).use { host ->
                    again.await<HostState.Authenticating>()
                    assertEquals(HostState.Connected(0u), again.await<HostState.Connected>())
                    host.disconnect()
                    assertEquals(CloseReason.Disconnected, again.await<HostState.Closed>().reason)
                }
                val unrelated = generateEd25519Key("")
                val changed = HostRecorder()
                try {
                    connectHost(request(fixture, key.privateKey.copyOf(), listOf(unrelated.publicKey.openssh)), changed).use { host ->
                        val prompt = changed.await<HostState.AwaitingHostKeyDecision>()
                        assertEquals(unrelated.publicKey.openssh, prompt.previouslyTrusted.single().openssh)
                        assertEquals(fixture.fingerprint, prompt.presented.fingerprint)
                        host.rejectHostKey()
                        val closed = changed.await<HostState.Closed>()
                        assertEquals(CloseReason.Failed(SessionFailure.HostKeyRejected), closed.reason)
                    }
                } finally {
                    unrelated.privateKey.fill(0)
                }

                // An unauthorized key never connects.
                val stranger = generateEd25519Key("")
                val refused = HostRecorder()
                try {
                    connectHost(request(fixture, stranger.privateKey.copyOf(), listOf(trustedLine)), refused).use {
                        refused.await<HostState.Authenticating>()
                        assertEquals(
                            CloseReason.Failed(SessionFailure.AuthenticationRejected),
                            refused.await<HostState.Closed>().reason,
                        )
                    }
                } finally {
                    stranger.privateKey.fill(0)
                }
            } finally {
                key.privateKey.fill(0)
            }
        }
    }

    @Test
    fun connectHostRunsAMoshTerminalThroughTheRealFfiRoamsAndReportsHealth() {
        assumeSshd()
        assumeMosh()
        OpenSshFixture().use { fixture ->
            val key = generateEd25519Key("")
            try {
                fixture.directory.resolve("authorized").toFile().writeText(key.publicKey.openssh + "\n")
                val recorder = HostRecorder()
                connectHost(request(fixture, key.privateKey.copyOf()), recorder).use { host ->
                    val prompt = recorder.await<HostState.AwaitingHostKeyDecision>()
                    host.approveHostKey(prompt.presented.fingerprint)
                    recorder.await<HostState.Authenticating>()
                    assertEquals(HostState.Connected(0u), recorder.await<HostState.Connected>())

                    // The probe found the fixture's mosh-server: AUTO would pick mosh here.
                    assertTrue(runBlocking { host.capabilities() }.moshServer != null)
                    val listener = RecordingListener()
                    val session = host.openTerminal(TerminalTarget.Shell, TerminalTransport.MOSH, 80u, 24u, listener)
                    assertEquals(TerminalTransport.MOSH, session.transport())
                    assertEquals(SessionState.Connected, listener.awaitState<SessionState.Connected>())
                    val grid = mutableMapOf<Int, String>()
                    session.sendText("stty -echo; printf '\\033[2J\\033[H'; printf 'OR2-%s\\n' READY\n")
                    awaitText(session, listener, grid, "OR2-READY")
                    session.resize(97u, 31u)
                    session.sendText("printf 'SIZE:'; stty size\n")
                    awaitText(session, listener, grid, "SIZE:31 97")

                    // Link health arrives about once a second after Connected; a healthy link is fresh.
                    val health = listener.awaitHealth()
                    assertTrue("healthy link: $health", health.sinceHeardMs < 5000u)

                    // Roaming through the FFI: the session's own roam() and the process-wide
                    // network_changed(); output keeps flowing after each.
                    session.roam()
                    session.sendText("printf 'ROAM-%s\\n' ONE\n")
                    awaitText(session, listener, grid, "ROAM-ONE")
                    networkChanged()
                    session.sendText("printf 'ROAM-%s\\n' TWO\n")
                    awaitText(session, listener, grid, "ROAM-TWO")
                    assertEquals(HostState.Connected(0u), host.state())

                    // A user disconnect of the host closes the mosh session too, before the host.
                    host.disconnect()
                    assertEquals(CloseReason.Disconnected, listener.awaitState<SessionState.Closed>().reason)
                    assertEquals(CloseReason.Disconnected, recorder.await<HostState.Closed>().reason)
                    listener.assertNoMoreStates()
                    assertFalse(listener.overlapped)
                }
            } finally {
                key.privateKey.fill(0)
            }
        }
    }

    private fun awaitText(
        session: Session,
        listener: RecordingListener,
        grid: MutableMap<Int, String>,
        expected: String,
    ): TerminalFrame {
        val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(5)
        while (System.nanoTime() < deadline) {
            val frame = listener.awaitFrame(session, quiescent = false)
            if (frame.full) grid.clear()
            frame.changedRows.forEach { row ->
                grid[row.index.toInt()] = row.cells.joinToString("") { cell ->
                    if (cell.width == CellWidth.SPACER_TAIL) "" else cell.text.ifEmpty { " " }
                }
            }
            if (grid.toSortedMap().values.joinToString("\n").contains(expected)) return frame
        }
        throw AssertionError("expected shell output did not arrive in terminal frames")
    }
}
