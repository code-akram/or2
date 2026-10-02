package io.github.code_akram.or2.connection

import io.github.code_akram.or2.data.TransportPref
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.HostException
import io.github.code_akram.or2.ffi.HostListener
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.paste.ImageFormat
import io.github.code_akram.or2.paste.PreparedImage
import io.github.code_akram.or2.paste.UploadState
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Image paste in the holder (contracts.md, "Image paste"): each terminal uploads over its host's
 * current connection, and a shared image is offered to the open terminals, the last used first.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class HostConnectionsImagePasteTest {
    private class Rig(val holder: HostConnections, val port: FakePort, val active: ActiveHost, val listener: () -> HostListener)

    private suspend fun TestScope.rig(): Rig {
        val host = testHost(transport = TransportPref.SSH)
        val port = FakePort()
        var listener: HostListener? = null
        val holder = HostConnections({ _, l -> listener = l; port }, FakeTrust(), StandardTestDispatcher(testScheduler),
            UnconfinedTestDispatcher(testScheduler))
        holder.connect(host, byteArrayOf(1))
        port.nativeState = HostState.Connected(0u)
        listener!!.onHostStateChanged(HostState.Connected(0u))
        advanceUntilIdle()
        return Rig(holder, port, holder.host(host.id)!!) { listener!! }
    }

    private fun TestScope.connected(rig: Rig, index: Int) {
        rig.port.terminals[index].second.onStateChanged(SessionState.Connected)
        advanceUntilIdle()
    }

    private val image = PreparedImage(ByteArray(5), ImageFormat.PNG)

    @Test
    fun aTerminalUploadsOverItsHostsConnectionAndGetsThePathBack() = runTest {
        val rig = rig()
        val terminal = rig.holder.openTerminal(rig.active, TerminalTarget.Shell)
        connected(rig, 0)
        assertNotNull(terminal.imagePaste)
        val paste = terminal.imagePaste!!
        assertTrue(paste.start { image })
        advanceUntilIdle()
        assertEquals(listOf("png" to 5), rig.port.uploads)
        assertEquals("/home/u/.cache/or2/images/or2-1.png", paste.paths.first())
        assertEquals(UploadState.Idle, paste.state.value)
    }

    @Test
    fun aHostThatIsNoLongerConnectedFailsTheUploadInWords() = runTest {
        val rig = rig()
        val terminal = rig.holder.openTerminal(rig.active, TerminalTarget.Shell)
        connected(rig, 0)
        rig.port.uploadFailure = HostException.Closed()
        terminal.imagePaste!!.start { image }
        advanceUntilIdle()
        assertEquals(UploadState.Failed("Not sent: the host is not connected"), terminal.imagePaste!!.state.value)
        // A host the holder no longer has.
        rig.holder.disconnect(rig.active.host.id)
        rig.listener().onHostStateChanged(HostState.Closed(CloseReason.Disconnected))
        advanceUntilIdle()
        rig.holder.dismissHost(rig.active.host.id)
        advanceUntilIdle()
        val failure = runCatching { rig.holder.uploadImage(terminal, ByteArray(1), "png") }.exceptionOrNull()
        assertTrue(failure.toString(), failure is HostException.Closed)
    }

    @Test
    fun closingATerminalCancelsItsUpload() = runTest {
        val rig = rig()
        val terminal = rig.holder.openTerminal(rig.active, TerminalTarget.Shell)
        connected(rig, 0)
        rig.port.uploadGate = CompletableDeferred()
        terminal.imagePaste!!.start { image }
        advanceUntilIdle()
        assertEquals(UploadState.Uploading, terminal.imagePaste!!.state.value)
        rig.holder.dismissTerminal(terminal)
        advanceUntilIdle()
        assertEquals(UploadState.Idle, terminal.imagePaste!!.state.value)
    }

    @Test
    fun aSharedImageIsOfferedToTheOpenTerminalsTheLastShownFirst() = runTest {
        val rig = rig()
        val first = rig.holder.openTerminal(rig.active, TerminalTarget.Shell)
        val second = rig.holder.openTerminal(rig.active, TerminalTarget.Tmux("main"))
        val third = rig.holder.openTerminal(rig.active, TerminalTarget.Tmux("build"))
        val fourth = rig.holder.openTerminal(rig.active, TerminalTarget.Shell)
        rig.port.terminals.indices.forEach { connected(rig, it) }
        // Never shown: the order they were opened in.
        assertEquals(listOf(first, second, third, fourth), rig.holder.shareTargets())
        rig.holder.attachDisplay(second)
        rig.holder.detachDisplay(second)
        rig.holder.attachDisplay(first)
        rig.holder.detachDisplay(first)
        advanceUntilIdle()
        assertEquals(listOf(first, second, third, fourth), rig.holder.shareTargets())
        rig.holder.attachDisplay(third)
        assertEquals(listOf(third, first, second, fourth), rig.holder.shareTargets())
        rig.holder.detachDisplay(third)
        // A closed terminal and one being closed take no image.
        rig.port.terminals[0].second.onStateChanged(SessionState.Closed(CloseReason.Disconnected))
        rig.holder.disconnectTerminal(second)
        advanceUntilIdle()
        assertEquals(listOf(third, fourth), rig.holder.shareTargets())
        // A connecting one neither.
        rig.holder.openTerminal(rig.active, TerminalTarget.Shell)
        assertEquals(listOf(third, fourth), rig.holder.shareTargets())
    }
}
