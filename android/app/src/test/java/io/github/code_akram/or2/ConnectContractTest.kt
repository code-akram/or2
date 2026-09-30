package io.github.code_akram.or2

import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.ConnectException
import io.github.code_akram.or2.ffi.ConnectRequest
import io.github.code_akram.or2.ffi.CellWidth
import io.github.code_akram.or2.ffi.KeyInput
import io.github.code_akram.or2.ffi.KeyModifiers
import io.github.code_akram.or2.ffi.Session
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalFrame
import io.github.code_akram.or2.ffi.TerminalKey
import io.github.code_akram.or2.ffi.connect
import io.github.code_akram.or2.ffi.generateEd25519Key
import java.net.InetAddress
import java.net.ServerSocket
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.attribute.PosixFilePermissions
import java.util.concurrent.TimeUnit
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test

class ConnectContractTest {
    @Test
    fun validatesBeforeStartingAnyNetworkWork() {
        val listener = RecordingListener()
        val key = generateEd25519Key("")
        try {
            assertThrows(ConnectException.InvalidHost::class.java) {
                connect(ConnectRequest("", 22u, "fixture", key.privateKey, emptyList(), 80u, 24u), listener)
            }
            listener.assertNoMoreStates()
        } finally {
            key.privateKey.fill(0)
        }
    }

    @Test
    fun realOpenSshShellThroughUniFfi() {
        assumeTrue("sshd is not installed", Files.isExecutable(Path.of("/usr/bin/sshd")))
        OpenSshFixture().use { fixture ->
            val listener = RecordingListener()
            val key = generateEd25519Key("")
            fixture.directory.resolve("authorized").toFile().writeText(key.publicKey.openssh + "\n")
            val session = try {
                connect(
                    ConnectRequest(
                        checkNotNull(InetAddress.getLoopbackAddress().hostAddress),
                        fixture.port.toUShort(), checkNotNull(System.getProperty("user.name")),
                        key.privateKey, emptyList(), 79u, 23u,
                    ),
                    listener,
                )
            } finally {
                key.privateKey.fill(0)
            }
            session.use {
                val prompt = listener.awaitState<SessionState.AwaitingHostKeyDecision>()
                assertEquals(fixture.fingerprint, prompt.presented.fingerprint)
                assertEquals(emptyList<Any>(), prompt.previouslyTrusted)
                it.resize(93u, 37u)
                it.approveHostKey(prompt.presented.fingerprint)
                listener.awaitState<SessionState.Authenticating>()
                listener.awaitState<SessionState.Connected>()
                val grid = mutableMapOf<Int, String>()
                it.sendText("stty -echo; printf '\\033[2J\\033[H'; printf 'OR2-%s\\n' READY\n")
                val first = awaitText(it, listener, grid, "OR2-READY")
                assertEquals(93.toUShort(), first.columns)
                assertEquals(37.toUShort(), first.rows)
                it.resize(101u, 41u)
                it.sendText("printf 'SIZE:'; stty size\n")
                val resized = awaitText(it, listener, grid, "SIZE:41 101")
                assertEquals(101.toUShort(), resized.columns)
                assertEquals(41.toUShort(), resized.rows)
                it.sendText("printf 'UTF-%s\\n' 'é界😀'\n")
                awaitText(it, listener, grid, "UTF-é界😀")
                it.sendText("printf 'KEY-%s\\n' ")
                it.sendKey(KeyInput(TerminalKey.Character("a"), KeyModifiers(false, false, false, false)))
                it.sendKey(KeyInput(TerminalKey.Enter, KeyModifiers(false, false, false, false)))
                awaitText(it, listener, grid, "KEY-a")
                it.sendText("exit 17\n")
                val reason = listener.awaitState<SessionState.Closed>().reason
                assertEquals(CloseReason.RemoteExited(17u), reason)
                listener.assertNoMoreStates()
                assertFalse(listener.overlapped)
                assertTrue(listener.callbackThreads.none { thread -> thread == Thread.currentThread() })
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

/** No system configuration, home keys or existing authorized_keys are read or changed. */
internal class OpenSshFixture : AutoCloseable {
    val directory: Path = Files.createTempDirectory("or2-sshd-")
    val port: Int = ServerSocket(0, 1, InetAddress.getLoopbackAddress()).use { it.localPort }
    val fingerprint: String
    private var process: Process? = null

    init {
        try {
            val host = generateEd25519Key("")
            fingerprint = host.publicKey.fingerprint
            try {
                Files.write(directory.resolve("host_key"), host.privateKey)
                Files.setPosixFilePermissions(directory.resolve("host_key"), PosixFilePermissions.fromString("rw-------"))
            } finally {
                host.privateKey.fill(0)
            }
            directory.resolve("authorized").toFile().writeText("")
            val config = directory.resolve("sshd_config")
            config.toFile().writeText(
                """
                Port $port
                ListenAddress ${InetAddress.getLoopbackAddress().hostAddress}
                HostKey ${directory.resolve("host_key")}
                AuthorizedKeysFile ${directory.resolve("authorized")}
                PidFile ${directory.resolve("pid")}
                StrictModes no
                UsePAM no
                PasswordAuthentication no
                KbdInteractiveAuthentication no
                PubkeyAuthentication yes
                PrintMotd no
                PrintLastLog no
                SetEnv HOME=$directory HISTFILE=/dev/null ENV=/dev/null BASH_ENV=/dev/null ZDOTDIR=$directory
                LogLevel VERBOSE
                """.trimIndent() + "\n",
            )
            val log = directory.resolve("log")
            process = ProcessBuilder("/usr/bin/sshd", "-D", "-e", "-f", config.toString())
                .redirectErrorStream(true).redirectOutput(log.toFile()).start()
            val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(5)
            while (!log.toFile().readText().contains("Server listening on")) {
                check(process!!.isAlive && System.nanoTime() < deadline) {
                    "disposable sshd did not start: ${log.toFile().readText()}"
                }
                Thread.sleep(20)
            }
        } catch (failure: Throwable) {
            close()
            throw failure
        }
    }

    override fun close() {
        process?.let {
            it.destroy()
            if (!it.waitFor(2, TimeUnit.SECONDS)) {
                it.destroyForcibly()
                it.waitFor(2, TimeUnit.SECONDS)
            }
        }
        directory.toFile().deleteRecursively()
    }
}
