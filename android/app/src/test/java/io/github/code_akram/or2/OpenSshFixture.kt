package io.github.code_akram.or2

import io.github.code_akram.or2.ffi.generateEd25519Key
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import java.net.InetAddress
import java.net.ServerSocket
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.attribute.PosixFilePermissions
import java.util.concurrent.TimeUnit

/**
 * No system configuration, home keys or existing authorized_keys are read or changed. Sessions
 * of this sshd are hermetic too: `TMUX_TMPDIR` is a private directory (a tmux started here never
 * touches the user's server), `$HOME` is the fixture directory, and `PATH` starts with a fake
 * `herdr` whose session list is empty, so the holder's automatic capability probe and watches can
 * never find or subscribe to a real herdr session.
 */
internal class OpenSshFixture : AutoCloseable {
    private companion object {
        const val BIND_ATTEMPTS = 5
    }

    val directory: Path = Files.createTempDirectory("or2-sshd-")
    val port: Int
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
            Files.createDirectory(directory.resolve("tmux")) // Private tmux sockets: never the default.
            val bin = Files.createDirectories(directory.resolve(".local/bin"))
            bin.resolve("herdr").toFile().apply {
                // No sessions, and nothing else works.
                writeText(
                    "#!/bin/sh\n" +
                        "if [ \"\$1 \$2\" = \"session list\" ]; then echo '{\"sessions\":[]}'; exit 0; fi\n" +
                        "echo \"fake herdr: \$*\" >&2\n" +
                        "exit 1\n",
                )
                setExecutable(true)
            }
            val settings = """
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
                SetEnv HOME=$directory HISTFILE=/dev/null ENV=/dev/null BASH_ENV=/dev/null ZDOTDIR=$directory TMUX_TMPDIR=${directory.resolve("tmux")} PATH=$bin:/usr/bin:/bin
                LogLevel VERBOSE
                """.trimIndent() + "\n"
            // A free port is found by binding and releasing it, so another process (often a
            // loopback client's ephemeral port) can take it before sshd binds. sshd then exits
            // with "Cannot bind any address": start it again on a new port.
            val config = directory.resolve("sshd_config")
            val log = directory.resolve("log")
            var bound = 0
            for (attempt in 1..BIND_ATTEMPTS) {
                val candidate = ServerSocket(0, 1, InetAddress.getLoopbackAddress()).use { it.localPort }
                config.toFile().writeText("Port $candidate\n$settings")
                val started = ProcessBuilder("/usr/bin/sshd", "-D", "-e", "-f", config.toString())
                    .redirectErrorStream(true).redirectOutput(log.toFile()).start()
                process = started
                val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(5)
                while (!log.toFile().readText().contains("Server listening on")) {
                    if (!started.isAlive) {
                        started.waitFor() // Everything sshd wrote is in the file once it exited.
                        val text = log.toFile().readText()
                        val bindFailed = "Address already in use" in text || "Cannot bind any address" in text
                        check(bindFailed && attempt < BIND_ATTEMPTS) { "disposable sshd did not start: $text" }
                        break
                    }
                    check(System.nanoTime() < deadline) {
                        "disposable sshd did not start: ${log.toFile().readText()}"
                    }
                    Thread.sleep(20)
                }
                if (started.isAlive) {
                    bound = candidate
                    break
                }
            }
            port = bound
        } catch (failure: Throwable) {
            close()
            throw failure
        }
    }

    /**
     * Cuts every established SSH connection by killing the process tree under the listener this
     * fixture started (the monitor and the session process of each connection; the listener
     * keeps accepting). A `mosh-server` a session started is a daemon that is no longer in that
     * tree, so it survives, as after a lost network.
     */
    fun dropConnections() {
        val listener = runCatching { directory.resolve("pid").toFile().readText().trim().toLong() }.getOrNull() ?: return
        val parents = mutableMapOf<Long, Long>()
        for (entry in Path.of("/proc").toFile().listFiles() ?: return) {
            val pid = entry.name.toLongOrNull() ?: continue
            // `pid (comm) state ppid ...`: the command name may hold spaces and brackets, so read after the last ')'.
            val stat = runCatching { entry.resolve("stat").readText() }.getOrNull() ?: continue
            stat.substringAfterLast(')').trim().split(' ').getOrNull(1)?.toLongOrNull()?.let { parents[pid] = it }
        }
        val tree = mutableSetOf<Long>()
        var frontier = setOf(listener)
        while (frontier.isNotEmpty()) {
            frontier = parents.filter { it.value in frontier && tree.add(it.key) }.keys
        }
        for (pid in tree) {
            runCatching { ProcessBuilder("kill", "-KILL", pid.toString()).start().waitFor(2, TimeUnit.SECONDS) }
        }
    }

    override fun close() {
        stopSessionProcesses()
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

/**
 * Kills the `mosh-server`s (and the shells under them) that this fixture's sessions started: a
 * mosh-server outlives its SSH connection by design, so a test that fails halfway would leave
 * one behind. Only processes carrying this fixture's private `TMUX_TMPDIR` are touched.
 */
private fun OpenSshFixture.stopSessionProcesses() {
    val marker = "TMUX_TMPDIR=${directory.resolve("tmux")}"
    val names = setOf("mosh-server", "bash", "zsh", "sh", "fish")
    val proc = Path.of("/proc").toFile().listFiles() ?: return
    for (entry in proc) {
        val pid = entry.name.toLongOrNull() ?: continue
        val comm = runCatching { entry.resolve("comm").readText().trim() }.getOrNull() ?: continue
        if (comm !in names) continue
        val environ = runCatching { entry.resolve("environ").readBytes() }.getOrNull() ?: continue
        if (String(environ, Charsets.ISO_8859_1).split('\u0000').none { it == marker }) continue
        runCatching { ProcessBuilder("kill", "-KILL", pid.toString()).start().waitFor(2, TimeUnit.SECONDS) }
    }
}

/**
 * Skips the calling test when `mosh-server` is absent, or fails it when `OR2_REQUIRE_MOSH` is set.
 */
internal fun assumeMosh() {
    val available = Files.isExecutable(Path.of("/usr/bin/mosh-server"))
    if (System.getenv("OR2_REQUIRE_MOSH") != null) {
        assertTrue("OR2_REQUIRE_MOSH is set but /usr/bin/mosh-server is not installed", available)
    }
    assumeTrue("mosh-server is not installed", available)
}

/**
 * Skips the calling test when `/usr/bin/sshd` is absent, or fails it when `OR2_REQUIRE_SSHD` is
 * set (CI), so the loopback-sshd suites are never vacuously green.
 */
internal fun assumeSshd() {
    val available = Files.isExecutable(Path.of("/usr/bin/sshd"))
    if (System.getenv("OR2_REQUIRE_SSHD") != null) {
        assertTrue("OR2_REQUIRE_SSHD is set but /usr/bin/sshd is not installed", available)
    }
    assumeTrue("sshd is not installed", available)
}
