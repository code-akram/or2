package io.github.code_akram.or2

import io.github.code_akram.or2.ffi.CellWidth
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.HostAddress
import io.github.code_akram.or2.ffi.HostConnectRequest
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.KeyInput
import io.github.code_akram.or2.ffi.KeyModifiers
import io.github.code_akram.or2.ffi.Session
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalFrame
import io.github.code_akram.or2.ffi.TerminalKey
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.ffi.connectHost
import io.github.code_akram.or2.ffi.generateEd25519Key
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Ignore
import org.junit.Test
import java.net.InetAddress
import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.TimeUnit

/**
 * The production `connect_host` against a disposable loopback OpenSSH. This replaces M1's
 * `connect` test: the same shell checks, now through `HostConnection.open_terminal`.
 */
class ConnectHostContractTest {
    @Ignore("enabled when lane A1 lands")
    @Test
    fun realOpenSshShellThroughUniFfi() {
        assumeTrue("sshd is not installed", Files.isExecutable(Path.of("/usr/bin/sshd")))
        OpenSshFixture().use { fixture ->
            val hostListener = HostRecorder()
            val key = generateEd25519Key("")
            fixture.directory.resolve("authorized").toFile().writeText(key.publicKey.openssh + "\n")
            val host = try {
                connectHost(
                    HostConnectRequest(
                        listOf(HostAddress(checkNotNull(InetAddress.getLoopbackAddress().hostAddress), fixture.port.toUShort())),
                        checkNotNull(System.getProperty("user.name")), key.privateKey, emptyList(),
                    ),
                    hostListener,
                )
            } finally {
                key.privateKey.fill(0)
            }
            host.use {
                val prompt = hostListener.await<HostState.AwaitingHostKeyDecision>()
                assertEquals(fixture.fingerprint, prompt.presented.fingerprint)
                assertEquals(emptyList<Any>(), prompt.previouslyTrusted)
                it.approveHostKey(prompt.presented.fingerprint)
                hostListener.await<HostState.Authenticating>()
                hostListener.await<HostState.Connected>()
                val listener = RecordingListener()
                val session = it.openTerminal(TerminalTarget.Shell, 93u, 37u, listener)
                session.use { shell ->
                    listener.awaitState<SessionState.Connected>()
                    val grid = mutableMapOf<Int, String>()
                    shell.sendText("stty -echo; printf '\\033[2J\\033[H'; printf 'OR2-%s\\n' READY\n")
                    val first = awaitText(shell, listener, grid, "OR2-READY")
                    assertEquals(93.toUShort(), first.columns)
                    assertEquals(37.toUShort(), first.rows)
                    shell.resize(101u, 41u)
                    shell.sendText("printf 'SIZE:'; stty size\n")
                    val resized = awaitText(shell, listener, grid, "SIZE:41 101")
                    assertEquals(101.toUShort(), resized.columns)
                    assertEquals(41.toUShort(), resized.rows)
                    shell.sendText("printf 'UTF-%s\\n' 'é界😀'\n")
                    awaitText(shell, listener, grid, "UTF-é界😀")
                    shell.sendText("printf 'KEY-%s\\n' ")
                    shell.sendKey(KeyInput(TerminalKey.Character("a"), KeyModifiers(false, false, false, false)))
                    shell.sendKey(KeyInput(TerminalKey.Enter, KeyModifiers(false, false, false, false)))
                    awaitText(shell, listener, grid, "KEY-a")
                    shell.sendText("exit 17\n")
                    assertEquals(CloseReason.RemoteExited(17u), listener.awaitState<SessionState.Closed>().reason)
                    listener.assertNoMoreStates()
                    assertFalse(listener.overlapped)
                    assertTrue(listener.callbackThreads.none { thread -> thread == Thread.currentThread() })
                }
                // The shell exiting ends only its channel; the host stays connected.
                assertEquals(HostState.Connected(0u), it.state())
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
