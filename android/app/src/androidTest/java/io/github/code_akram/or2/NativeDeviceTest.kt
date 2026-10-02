package io.github.code_akram.or2

import androidx.test.ext.junit.runners.AndroidJUnit4
import io.github.code_akram.or2.ffi.CellWidth
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.ConnectException
import io.github.code_akram.or2.ffi.ConnectRequest
import io.github.code_akram.or2.ffi.HostAddress
import io.github.code_akram.or2.ffi.HostConnectRequest
import io.github.code_akram.or2.ffi.HostListener
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.KeyException
import io.github.code_akram.or2.ffi.LinkHealth
import io.github.code_akram.or2.ffi.Renderer
import io.github.code_akram.or2.ffi.SessionListener
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalException
import io.github.code_akram.or2.ffi.buildInfo
import io.github.code_akram.or2.ffi.contractProbeHost
import io.github.code_akram.or2.ffi.contractProbeSession
import io.github.code_akram.or2.ffi.generateEd25519Key
import io.github.code_akram.or2.ffi.importPrivateKey
import io.github.code_akram.or2.ffi.terminalSize
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class NativeDeviceTest {
    @Test
    fun loadsPackagedArm64LibraryAndRoundTripsThroughUniFfi() {
        val info = buildInfo()
        assertEquals(15u, info.apiVersion)
        assertEquals(34u, info.minimumAndroidSdk)
        assertEquals(Renderer.CANVAS, info.renderer)
        val size = terminalSize(97u, 31u)
        assertEquals(97.toUShort(), size.columns)
        assertEquals(31.toUShort(), size.rows)
        assertEquals(3007u, size.cellCount)
        assertEquals(4294836225u, terminalSize(65535u, 65535u).cellCount)
        assertThrows(TerminalException.EmptyDimension::class.java) { terminalSize(0u, 31u) }
        assertThrows(TerminalException.EmptyDimension::class.java) { terminalSize(97u, 0u) }
    }

    @Test
    fun generatesAndReimportsKeysWithTypedErrors() {
        val key = generateEd25519Key("device")
        assertEquals("ssh-ed25519", key.publicKey.algorithm)
        assertTrue(key.publicKey.fingerprint.startsWith("SHA256:"))
        assertEquals(key.publicKey, importPrivateKey(key.privateKey, null).publicKey)
        assertThrows(KeyException.Malformed::class.java) { importPrivateKey(byteArrayOf(1, 2, 3), null) }
    }

    private class Listener : SessionListener {
        val states = LinkedBlockingQueue<SessionState>()
        val frames = LinkedBlockingQueue<Thread>()
        val threads = LinkedBlockingQueue<Thread>()

        override fun onStateChanged(state: SessionState) {
            threads.add(Thread.currentThread())
            states.add(state)
        }

        override fun onFrameReady() {
            threads.add(Thread.currentThread())
            frames.add(Thread.currentThread())
        }

        override fun onLinkHealth(health: LinkHealth) = Unit

        override fun onClipboardWrite(text: String) = Unit
        override fun onServerPid(pid: UInt) = Unit

        fun next(): SessionState {
            val state = states.poll(5, TimeUnit.SECONDS)
            assertNotNull("timed out waiting for a state change", state)
            return state!!
        }
    }

    @Test
    fun deliversSessionCallbacksFromRustThreadsOnArt() {
        val key = generateEd25519Key("device")
        val listener = Listener()
        val request = ConnectRequest("probe.invalid", 22u, "akram", key.privateKey, emptyList(), 12u, 3u)
        assertThrows(ConnectException.InvalidPort::class.java) {
            contractProbeSession(request.copy(port = 0u), listener)
        }
        contractProbeSession(request, listener).use { session ->
            val prompt = listener.next() as SessionState.AwaitingHostKeyDecision
            session.approveHostKey(prompt.presented.fingerprint)
            assertEquals(SessionState.Authenticating, listener.next())
            assertEquals(SessionState.Connected, listener.next())
            assertNotNull(listener.frames.poll(5, TimeUnit.SECONDS))
            val frame = session.takeFrame()!!
            assertTrue(frame.full)
            val row = frame.changedRows[1].cells
            assertEquals(12, row.size)
            assertEquals(listOf("R", "界", "", "😀", ""), row.take(5).map { it.text })
            assertEquals(CellWidth.SPACER_TAIL, row[2].width)
            session.disconnect()
            assertEquals(SessionState.Closed(CloseReason.Disconnected), listener.next())
        }
        assertFalse(Thread.currentThread() in listener.threads)
    }

    @Test
    fun answersHostQueriesThroughUniFfiAsyncOnArt() {
        val key = generateEd25519Key("device")
        val states = LinkedBlockingQueue<HostState>()
        val listener = object : HostListener {
            override fun onHostStateChanged(state: HostState) {
                states.add(state)
            }
        }
        val request = HostConnectRequest(listOf(HostAddress("probe.invalid", 22u)), "akram", key.privateKey, emptyList())
        contractProbeHost(request, listener).use { host ->
            val prompt = states.poll(5, TimeUnit.SECONDS) as HostState.AwaitingHostKeyDecision
            host.approveHostKey(prompt.presented.fingerprint)
            assertEquals(HostState.Authenticating, states.poll(5, TimeUnit.SECONDS))
            assertEquals(HostState.Connected(0u), states.poll(5, TimeUnit.SECONDS))
            val capabilities = runBlocking { host.capabilities() }
            assertEquals("/usr/bin/tmux", capabilities.tmux)
            assertEquals(listOf("main", "build"), runBlocking { host.listTmuxSessions() }.map { it.name })
            host.disconnect()
            assertEquals(HostState.Closed(CloseReason.Disconnected), states.poll(5, TimeUnit.SECONDS))
        }
    }
}
