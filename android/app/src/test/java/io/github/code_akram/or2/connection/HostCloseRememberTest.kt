package io.github.code_akram.or2.connection

import io.github.code_akram.or2.app.MemoryPrefStore
import io.github.code_akram.or2.app.ReattachMemory
import io.github.code_akram.or2.app.rememberShown
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.HostEndpoint
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.HostListener
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.SessionFailure
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.ffi.TerminalTransport
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * A host-wide close (the user's Disconnect, an edit of the destination or login) must keep every
 * terminal of that host from becoming the Resume target again before the native `Closed` arrives:
 * the terminal screen's remembering effect can be queued, or run again on a revisit, while the
 * terminal still reads `Connected`. All connection generations count, including older ones that
 * were retained for their surviving mosh terminals. The final frames stay; a terminal that really
 * survives an SSH loss stays eligible. On fakes; the effect is the production helper `rememberShown`.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class HostCloseRememberTest {
    private class Rig(val scope: TestScope) {
        val host: Host = testHost()
        val memory = ReattachMemory(MemoryPrefStore())
        val ports = mutableListOf<FakePort>()
        val listeners = mutableListOf<HostListener>()
        val holder = HostConnections(
            { _, listener ->
                listeners += listener
                FakePort().also {
                    it.caps = it.caps.copy(moshServer = "/usr/bin/mosh-server")
                    it.nextServerPid = 4242u
                    ports += it
                }
            },
            FakeTrust(), StandardTestDispatcher(scope.testScheduler), UnconfinedTestDispatcher(scope.testScheduler),
            moshServers = MoshServerLedger(MemoryPrefStore()),
        ).also { it.userClose = memory }

        suspend fun connect() {
            holder.connect(host, byteArrayOf(1))
            ports.last().nativeState = HostState.Connected(0u)
            listeners.last().onHostStateChanged(HostState.Connected(0u))
            scope.advanceUntilIdle()
        }

        fun lose() {
            listeners.last().onHostStateChanged(HostState.Closed(CloseReason.Failed(SessionFailure.ConnectionLost("reset"))))
            scope.advanceUntilIdle()
        }

        fun open(target: TerminalTarget = TerminalTarget.Shell): ActiveTerminal {
            val terminal = holder.openTerminal(holder.host(host.id)!!, target)
            ports.last().terminals.last().second.onStateChanged(SessionState.Connected)
            scope.advanceUntilIdle()
            return terminal
        }

        fun nativeClosed(port: Int, terminal: Int) {
            ports[port].terminals[terminal].second.onStateChanged(SessionState.Closed(CloseReason.Disconnected))
            scope.advanceUntilIdle()
        }

        /** The screen's remembering effect, as it runs for [terminal] right now. */
        fun effect(terminal: ActiveTerminal) = memory.rememberShown(terminal, terminal.transport.value)

        fun assertFinalFrameKept(terminal: ActiveTerminal) {
            assertTrue(terminal in holder.terminals.value)
            assertNotNull(terminal.handle.value)
        }
    }

    @Test
    fun aDestinationEditMustKeepAQueuedRememberEffectFromRestoringTheOldTarget() = runTest {
        val host = testHost()
        val memory = ReattachMemory(MemoryPrefStore())
        val port = FakePort()
        lateinit var listener: HostListener
        val holder = HostConnections({ _, l -> listener = l; port }, FakeTrust(),
            StandardTestDispatcher(testScheduler), UnconfinedTestDispatcher(testScheduler))
        holder.userClose = memory
        holder.connect(host, byteArrayOf(1))
        port.nativeState = HostState.Connected(0u)
        listener.onHostStateChanged(HostState.Connected(0u))
        advanceUntilIdle()
        val terminal = holder.openTerminal(holder.host(host.id)!!, TerminalTarget.Herdr("old-work", "w1:p2"))
        port.terminals.last().second.onStateChanged(SessionState.Connected)
        advanceUntilIdle()
        memory.rememberShown(terminal, terminal.transport.value)
        holder.hostEdited(host, host.copy(addresses = listOf(HostEndpoint("different.invalid", 22))))
        assertNull(memory.last.value)
        // A pending screen effect runs before native Closed reaches the holder.
        memory.rememberShown(terminal, terminal.transport.value)
        assertNull(memory.last.value)
        port.terminals.last().second.onStateChanged(SessionState.Closed(CloseReason.Disconnected))
        advanceUntilIdle()
        assertNull("an edit's invalidated Resume target was restored by the old connected terminal", memory.last.value)
    }

    @Test
    fun aHostDisconnectMustKeepAQueuedRememberEffectFromRestoringTheTarget() = runTest {
        val host = testHost()
        val memory = ReattachMemory(MemoryPrefStore())
        val port = FakePort()
        lateinit var listener: HostListener
        val holder = HostConnections({ _, l -> listener = l; port }, FakeTrust(),
            StandardTestDispatcher(testScheduler), UnconfinedTestDispatcher(testScheduler))
        holder.userClose = memory
        holder.connect(host, byteArrayOf(1))
        port.nativeState = HostState.Connected(0u)
        listener.onHostStateChanged(HostState.Connected(0u))
        advanceUntilIdle()
        val terminal = holder.openTerminal(holder.host(host.id)!!, TerminalTarget.Shell)
        port.terminals.last().second.onStateChanged(SessionState.Connected)
        advanceUntilIdle()
        memory.rememberShown(terminal, terminal.transport.value)
        holder.disconnect(host.id)
        assertNull(memory.last.value)
        memory.rememberShown(terminal, terminal.transport.value)
        assertNull(memory.last.value)
        // Disconnected does not forget again: nothing may have been restored to survive Closed.
        port.terminals.last().second.onStateChanged(SessionState.Closed(CloseReason.Disconnected))
        advanceUntilIdle()
        assertNull("host disconnect resurrected Resume memory", memory.last.value)
    }

    @Test
    fun aHostDisconnectAlsoBlocksATerminalOfAnOlderRetainedConnection() = runTest {
        val rig = Rig(this)
        rig.connect()
        val old = rig.open()
        rig.lose()
        rig.connect()
        val current = rig.open(TerminalTarget.Tmux("work"))
        // Both are connected and eligible: the old one survived the SSH loss.
        assertTrue(old.isOpenForReattach && current.isOpenForReattach)
        rig.effect(old)
        assertNotNull(rig.memory.last.value)

        rig.holder.disconnect(rig.host.id)
        assertNull(rig.memory.last.value)
        assertFalse(old.isOpenForReattach)
        assertFalse(current.isOpenForReattach)
        // The queued or revisited effects of both generations run before any native Closed.
        rig.effect(old)
        rig.effect(current)
        assertNull(rig.memory.last.value)
        rig.assertFinalFrameKept(old)
        rig.assertFinalFrameKept(current)

        rig.nativeClosed(0, 0)
        rig.nativeClosed(1, 0)
        rig.effect(old)
        rig.effect(current)
        assertNull("a closed terminal of an older generation restored Resume memory", rig.memory.last.value)
        rig.assertFinalFrameKept(old)
    }

    @Test
    fun aHostDisconnectBlocksAnOlderTerminalWhenTheCurrentConnectionIsAlreadyGone() = runTest {
        val rig = Rig(this)
        rig.connect()
        val old = rig.open()
        rig.lose()
        rig.connect()
        // The replacement is lost too and nothing replaced it: the survivor is on a retained generation.
        rig.lose()
        rig.holder.disconnect(rig.host.id)
        assertFalse(old.isOpenForReattach)
        rig.effect(old)
        assertNull(rig.memory.last.value)
        rig.nativeClosed(0, 0)
        rig.effect(old)
        assertNull(rig.memory.last.value)
    }

    @Test
    fun aDestinationEditBlocksTerminalsOfEveryGeneration() = runTest {
        val rig = Rig(this)
        rig.connect()
        val old = rig.open(TerminalTarget.Herdr("old-work", "w1:p2"))
        rig.lose()
        rig.connect()
        val current = rig.open()
        rig.effect(old)
        assertNotNull(rig.memory.last.value)

        rig.holder.hostEdited(rig.host, rig.host.copy(addresses = listOf(HostEndpoint("different.invalid", 22))))
        assertNull(rig.memory.last.value)
        assertFalse(old.isOpenForReattach)
        assertFalse(current.isOpenForReattach)
        rig.effect(old)
        rig.effect(current)
        assertNull(rig.memory.last.value)
        rig.assertFinalFrameKept(old)
        rig.assertFinalFrameKept(current)

        rig.nativeClosed(0, 0)
        rig.nativeClosed(1, 0)
        rig.effect(old)
        rig.effect(current)
        assertNull("the edit's old terminals restored the invalidated target", rig.memory.last.value)
    }

    @Test
    fun aLoginEditBlocksTheSameWay() = runTest {
        val rig = Rig(this)
        rig.connect()
        val terminal = rig.open()
        rig.effect(terminal)
        val edited = Host(rig.host.record.copy(username = "someone-else"), rig.host.addresses)
        rig.holder.hostEdited(rig.host, edited)
        assertNull(rig.memory.last.value)
        rig.effect(terminal)
        assertNull(rig.memory.last.value)
        rig.nativeClosed(0, 0)
        rig.effect(terminal)
        assertNull(rig.memory.last.value)
    }

    @Test
    fun aLabelEditKeepsTheMemoryAndTheEligibility() = runTest {
        val rig = Rig(this)
        rig.connect()
        val terminal = rig.open()
        rig.effect(terminal)
        rig.holder.hostEdited(rig.host, Host(rig.host.record.copy(label = "Renamed"), rig.host.addresses))
        assertNotNull(rig.memory.last.value)
        assertTrue(terminal.isOpenForReattach)
    }

    @Test
    fun aSurvivingMoshTerminalStaysEligibleAfterSshLossAndReplacement() = runTest {
        val rig = Rig(this)
        rig.connect()
        val survivor = rig.open()
        assertEquals(TerminalTransport.MOSH, survivor.transport.value)
        rig.lose()
        assertTrue(survivor.isOpenForReattach)
        rig.connect()
        assertTrue("an ordinary SSH replacement made a surviving mosh terminal ineligible", survivor.isOpenForReattach)
        rig.effect(survivor)
        assertEquals(survivor.target, rig.memory.last.value?.target)
        // A survivor still counts after an unrelated terminal of the new connection is closed.
        val other = rig.open(TerminalTarget.Tmux("work"))
        rig.holder.disconnectTerminal(other)
        assertTrue(survivor.isOpenForReattach)
    }
}
