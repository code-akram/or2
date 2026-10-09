package io.github.code_akram.or2

import androidx.test.ext.junit.runners.AndroidJUnit4
import io.github.code_akram.or2.ffi.CellWidth
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.HostAddress
import io.github.code_akram.or2.ffi.HostConnectRequest
import io.github.code_akram.or2.ffi.HostListener
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.KeyException
import io.github.code_akram.or2.ffi.LinkHealth
import io.github.code_akram.or2.ffi.SessionException
import io.github.code_akram.or2.ffi.SessionListener
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.buildInfo
import io.github.code_akram.or2.ffi.contractProbeHost
import io.github.code_akram.or2.ffi.contractProbeSession
import io.github.code_akram.or2.ffi.generateEd25519Key
import io.github.code_akram.or2.ffi.importPrivateKey
import io.github.code_akram.or2.ffi.mergeDirectoryPaths
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
        assertEquals(24u, info.apiVersion)
        assertEquals("0.1.10", info.version)
    }

    @Test
    fun liveDirectoryMergingCrossesThePackagedFfiWithRustValidation() {
        assertEquals(
            listOf("/device/live", "/device/history"),
            mergeDirectoryPaths(listOf("/device/live", "/hidden\u202epath"), listOf("/device/live", "/device/history", "~/relative")),
        )
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
        val listener = Listener()
        assertThrows(SessionException.EmptyDimension::class.java) { contractProbeSession(0u, 3u, listener) }
        contractProbeSession(12u, 3u, listener).use { session ->
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
