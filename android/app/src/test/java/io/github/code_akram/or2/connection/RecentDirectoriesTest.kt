package io.github.code_akram.or2.connection

import io.github.code_akram.or2.data.TransportPref
import io.github.code_akram.or2.ffi.HostException
import io.github.code_akram.or2.ffi.HostListener
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.host.DirectoryList
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.*
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class RecentDirectoriesTest {
    @Test
    fun historyReadsDoNotGateConnectingOrShellsAndConcurrentRefreshesAreCoalesced() = runTest {
        val gate = CompletableDeferred<Unit>()
        val port = FakePort().apply { directories = listOf("/work/project"); directoriesGate = gate }
        lateinit var listener: HostListener
        val holder = HostConnections({ _, l -> listener = l; port }, FakeTrust(),
            StandardTestDispatcher(testScheduler), UnconfinedTestDispatcher(testScheduler))
        val host = testHost(transport = TransportPref.SSH)
        holder.connect(host, byteArrayOf(1))
        port.nativeState = HostState.Connected(0u)
        listener.onHostStateChanged(port.nativeState)
        runCurrent()
        val active = holder.host(host.id)!!
        assertTrue(holder.isLive(host.id))
        assertEquals(DirectoryList.Loading, active.directories.value)
        assertTrue(active.readingDirectories.value)
        val shell = holder.openTerminal(active, TerminalTarget.ShellIn("/work/project"))
        assertNull(shell.targetScroller) // Plain shell scrolling; no multiplexer gestures.
        holder.refreshDirectories(active)
        assertEquals(1, port.directoryCalls)
        gate.complete(Unit)
        runCurrent()
        assertEquals(DirectoryList.Loaded(listOf("/work/project")), active.directories.value)
        assertFalse(active.readingDirectories.value)
        port.directories = emptyList()
        holder.refreshDirectories(active)
        assertEquals(DirectoryList.Loaded(emptyList<String>()), active.directories.value)
        port.directoriesFailure = HostException.CommandFailed("history unavailable")
        holder.refreshDirectories(active)
        assertTrue(active.directories.value is DirectoryList.Failed)
        assertTrue(holder.isLive(host.id))
        holder.dismissHost(host.id)
    }

    @Test
    fun cancellationReleasesTheReadFlagWithoutDiscardingTheCachedList() = runTest {
        val port = FakePort().apply { directories = listOf("/work/project") }
        lateinit var listener: HostListener
        val holder = HostConnections({ _, l -> listener = l; port }, FakeTrust(),
            StandardTestDispatcher(testScheduler), UnconfinedTestDispatcher(testScheduler))
        val host = testHost()
        holder.connect(host, byteArrayOf(1))
        port.nativeState = HostState.Connected(0u)
        listener.onHostStateChanged(port.nativeState)
        runCurrent()
        val active = holder.host(host.id)!!
        port.directoriesGate = CompletableDeferred()
        val read = launch { holder.refreshDirectories(active) }
        runCurrent()
        assertTrue(active.readingDirectories.value)
        read.cancel()
        runCurrent()
        assertFalse(active.readingDirectories.value)
        assertEquals(DirectoryList.Loaded(listOf("/work/project")), active.directories.value)
        holder.dismissHost(host.id)
    }

    @Test
    fun aReplacedConnectionsLateAnswerCannotPopulateTheNewConnectionsCache() = runTest {
        val old = FakePort().apply { directories = listOf("/old"); directoriesGate = CompletableDeferred() }
        val fresh = FakePort().apply { directories = listOf("/fresh") }
        var next = old
        val listeners = mutableListOf<HostListener>()
        val holder = HostConnections({ _, l -> listeners += l; next }, FakeTrust(),
            StandardTestDispatcher(testScheduler), UnconfinedTestDispatcher(testScheduler))
        val host = testHost()
        holder.connect(host, byteArrayOf(1))
        old.nativeState = HostState.Connected(0u)
        listeners[0].onHostStateChanged(old.nativeState)
        runCurrent()
        holder.dismissHost(host.id)
        next = fresh
        holder.connect(host, byteArrayOf(2))
        fresh.nativeState = HostState.Connected(0u)
        listeners[1].onHostStateChanged(fresh.nativeState)
        runCurrent()
        old.directoriesGate!!.complete(Unit)
        runCurrent()
        assertEquals(DirectoryList.Loaded(listOf("/fresh")), holder.host(host.id)!!.directories.value)
        holder.dismissHost(host.id)
    }
}
