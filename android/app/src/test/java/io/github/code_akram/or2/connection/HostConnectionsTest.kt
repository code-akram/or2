package io.github.code_akram.or2.connection

import io.github.code_akram.or2.data.HostEndpoint
import io.github.code_akram.or2.data.TrustStore
import io.github.code_akram.or2.ffi.*
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.Job
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.*
import org.junit.Test
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import kotlin.coroutines.CoroutineContext

/**
 * The holder's host-key, trust, wiping and terminal-lifecycle rules, per host, on fakes.
 * (M1's session-holder cases carry over: the host is the connection that prompts and wipes.)
 */
@OptIn(ExperimentalCoroutinesApi::class)
class HostConnectionsTest {
    private val host = testHost()

    private fun TestScope.holder(
        trust: TrustStore = FakeTrust(), connector: HostConnector,
        main: CoroutineDispatcher = StandardTestDispatcher(testScheduler),
        worker: CoroutineDispatcher = UnconfinedTestDispatcher(testScheduler),
    ) = HostConnections(connector, trust, main, worker)

    private fun connected(port: FakePort) { port.nativeState = HostState.Connected(0u) }

    @Test
    fun earlyCallbacksAndApprovalWaitForAssignmentAndNeverTouchTerminals() = runTest {
        val store = FakeTrust()
        val port = FakePort(store.events)
        val bytes = byteArrayOf(9, 2, 5)
        lateinit var holder: HostConnections
        lateinit var approval: Job
        holder = holder(store, connector = { request, listener ->
            assertArrayEquals(byteArrayOf(9, 2, 5), request.privateKey) // Not wiped at construction.
            assertEquals(listOf("prior-line-one", "prior-line-two"), request.trustedHostKeys)
            assertEquals(listOf(HostAddress("fixture.invalid", 2222u)), request.addresses)
            assertEquals("fixture", request.username)
            port.nativeState = testPrompt
            listener.onHostStateChanged(testPrompt)
            testScheduler.runCurrent() // Main processes the callback while the factory has not returned.
            val current = holder.host(host.id)!!
            assertNull(current.mutablePort.value)
            assertEquals(testPrompt, current.state.value)
            approval = launch(start = CoroutineStart.UNDISPATCHED) { holder.approve(current, testPrompt) }
            assertTrue(store.events.isEmpty())
            port
        })
        holder.connect(host, bytes)
        approval.join()
        val current = holder.host(host.id)!!
        assertSame(port, current.mutablePort.value)
        assertEquals(listOf("persist", "approve"), store.events)
        assertEquals(testPublicKey.fingerprint, port.approved)
        assertEquals(host, store.persistedHost)
        assertEquals(listOf(testPublicKey.openssh), store.lines)
        assertArrayEquals(ByteArray(3), bytes)
        holder.disconnect(host.id)
        assertFalse(port.destroyed)
        holder.dismissHost(host.id)
        assertEquals(1, store.events.count { it == "close" })
    }

    @Test
    fun trustIsPersistedForTheHostWithItsFullAddressListAtApprovalTime() = runTest {
        val store = FakeTrust()
        val port = FakePort()
        val addresses = listOf(HostEndpoint("a.invalid", 22), HostEndpoint("b.invalid", 2022))
        val multi = testHost(addresses = addresses)
        lateinit var request: HostConnectRequest
        val holder = holder(store, connector = { r, _ -> request = r; port })
        holder.connect(multi, byteArrayOf(1))
        assertEquals(listOf(HostAddress("a.invalid", 22u), HostAddress("b.invalid", 2022u)), request.addresses)
        port.nativeState = testPrompt
        val current = holder.host(multi.id)!!
        current.mutableState.value = testPrompt
        holder.approve(current, testPrompt)
        assertEquals(addresses, store.persistedHost!!.addresses)
        holder.dismissHost(multi.id)
    }

    @Test
    fun terminalEligibilityRequiresConnectedAndSurvivesConflatedClosedButNotReplacement() = runTest {
        val listeners = mutableListOf<HostListener>()
        val holder = holder(connector = { _, listener -> listeners += listener; FakePort() })
        holder.connect(host, byteArrayOf(8))
        val failed = holder.host(host.id)!!
        listeners[0].onHostStateChanged(HostState.Authenticating)
        listeners[0].onHostStateChanged(HostState.Closed(CloseReason.Failed(SessionFailure.AuthenticationRejected)))
        runCurrent()
        assertNotNull(failed.mutablePort.value)
        assertFalse(failed.hasConnected.value) // A handle alone must not expose terminal controls.
        assertFalse(holder.isLive(host.id))
        holder.connect(host, byteArrayOf(9)) // A closed connection is replaced by a fresh one.
        val connectedHost = holder.host(host.id)!!
        assertNotSame(failed, connectedHost)
        listeners[1].onHostStateChanged(HostState.Connected(0u))
        listeners[1].onHostStateChanged(HostState.Closed(CloseReason.Disconnected))
        runCurrent() // The UI may observe only Closed, but the holder must remember Connected.
        assertEquals(HostState.Closed(CloseReason.Disconnected), connectedHost.state.value)
        assertTrue(connectedHost.hasConnected.value)
        holder.connect(host, byteArrayOf(7))
        assertFalse(holder.host(host.id)!!.hasConnected.value)
        holder.dismissHost(host.id)
    }

    @Test
    fun persistenceFailureAndExpiredPromptNeverApprove() = runTest {
        val dispatcher = UnconfinedTestDispatcher(testScheduler)
        val store = FakeTrust().apply { fail = true }
        val port = FakePort()
        lateinit var listener: HostListener
        val holder = HostConnections({ _, incoming -> listener = incoming; port }, store, dispatcher, dispatcher)
        holder.connect(host, byteArrayOf(3, 8))
        port.nativeState = testPrompt
        listener.onHostStateChanged(testPrompt)
        val current = holder.host(host.id)!!
        try { holder.approve(current, testPrompt); fail("Expected storage failure") } catch (_: IllegalStateException) { }
        assertNull(port.approved)
        assertEquals(listOf("prior-line-one", "prior-line-two"), store.lines)
        store.fail = false
        listener.onHostStateChanged(HostState.Closed(CloseReason.Failed(SessionFailure.TimedOut)))
        try { holder.approve(current, testPrompt); fail("Expected expired prompt") } catch (_: IllegalStateException) { }
        assertNull(port.approved)
        holder.disconnect(host.id)
    }

    @Test
    fun nativeClosedBeforeQueuedCallbackNeverStartsTrustPersistence() = runTest {
        val store = FakeTrust()
        val port = FakePort()
        lateinit var listener: HostListener
        val holder = holder(store, connector = { _, incoming -> listener = incoming; port })
        holder.connect(host, byteArrayOf(4, 2))
        port.nativeState = testPrompt
        listener.onHostStateChanged(testPrompt)
        runCurrent()
        val current = holder.host(host.id)!!
        val closed = HostState.Closed(CloseReason.Failed(SessionFailure.ConnectionLost("Fixture closed")))
        port.nativeState = closed
        listener.onHostStateChanged(closed) // Main delivery remains queued while Trust is tapped.
        assertEquals(testPrompt, current.state.value)
        val failure = runCatching { holder.approve(current, testPrompt) }.exceptionOrNull()
        assertEquals(0, store.replacements)
        assertEquals(listOf("prior-line-one", "prior-line-two"), store.lines)
        assertNull(port.approved)
        assertTrue(failure is IllegalStateException)
        assertEquals("Host-key prompt has expired.", failure!!.message)
        runCurrent()
        assertEquals(closed, current.state.value)
        holder.dismissHost(host.id)
    }

    @Test
    fun closeDuringPersistenceAndReplacedConnectionCallbacksAreIsolated() = runTest {
        val dispatcher = UnconfinedTestDispatcher(testScheduler)
        val store = FakeTrust().apply { gate = CompletableDeferred() }
        val ports = mutableListOf<FakePort>()
        val listeners = mutableListOf<HostListener>()
        val holder = HostConnections({ _, listener ->
            listeners += listener
            FakePort().also { ports += it }
        }, store, dispatcher, dispatcher)
        holder.connect(host, byteArrayOf(6))
        ports[0].nativeState = testPrompt
        listeners[0].onHostStateChanged(testPrompt)
        val previous = holder.host(host.id)!!
        val approval = launch(start = CoroutineStart.UNDISPATCHED) { holder.approve(previous, testPrompt) }
        assertEquals(1, store.replacements) // Write began while both native and cached prompt were live.
        ports[0].nativeState = HostState.Closed(CloseReason.Failed(SessionFailure.ConnectionLost("Fixture closed")))
        listeners[0].onHostStateChanged(ports[0].nativeState)
        store.gate!!.complete(Unit)
        approval.join()
        assertEquals(host, store.persistedHost)
        assertEquals(listOf(testPublicKey.openssh), store.lines) // An already-started write still completes.
        assertNull(ports[0].approved)
        holder.connect(host, byteArrayOf(7)) // Replaces the closed connection and retires its object.
        assertEquals(1, ports[0].events.count { it == "close" })
        listeners[0].onHostStateChanged(HostState.Connected(0u)) // A stale callback from the old connection.
        assertEquals(HostState.Connecting, holder.host(host.id)!!.state.value)
        holder.reject(holder.host(host.id)!!)
        assertTrue(ports[1].events.contains("reject"))
        holder.dismissHost(host.id)
    }

    @Test
    fun synchronousFactoryAndTrustReadFailuresStillWipe() = runTest {
        val dispatcher = UnconfinedTestDispatcher(testScheduler)
        val bytes = byteArrayOf(11, 4, 1)
        val holder = HostConnections({ request, _ ->
            assertArrayEquals(byteArrayOf(11, 4, 1), request.privateKey)
            throw HostConnectException.InvalidPrivateKey()
        }, FakeTrust(), dispatcher, dispatcher)
        try { holder.connect(host, bytes); fail("Expected validation error") } catch (_: HostConnectException.InvalidPrivateKey) { }
        assertArrayEquals(ByteArray(3), bytes)
        assertTrue(holder.hosts.value.isEmpty())
        val readFailure = object : TrustStore {
            override suspend fun trustedKeys(hostId: Long): List<String> = error("read failed")
            override suspend fun replaceTrust(host: io.github.code_akram.or2.data.Host, presented: PublicKeyInfo) = Unit
        }
        val other = HostConnections({ _, _ -> fail("Must not call factory"); FakePort() }, readFailure, dispatcher, dispatcher)
        val secret = byteArrayOf(3, 5)
        try { other.connect(host, secret); fail("Expected read failure") } catch (_: IllegalStateException) { }
        assertArrayEquals(ByteArray(2), secret)
    }

    @Test
    fun cancellationAtFactoryReturnWipesButDoesNotLeakTheNativeHandle() = runTest {
        val worker = Executors.newSingleThreadExecutor().asCoroutineDispatcher()
        val entered = CompletableDeferred<Unit>()
        val release = CountDownLatch(1)
        val port = FakePort()
        val bytes = byteArrayOf(4, 9, 2)
        val holder = holder(connector = { request, _ ->
            entered.complete(Unit)
            check(release.await(5, TimeUnit.SECONDS))
            assertArrayEquals(byteArrayOf(4, 9, 2), request.privateKey)
            port
        }, worker = worker)
        try {
            val job = launch { holder.connect(host, bytes) }
            runCurrent()
            entered.await()
            job.cancel()
            assertArrayEquals(byteArrayOf(4, 9, 2), bytes) // Factory still needs these bytes.
            release.countDown()
            job.join()
            assertArrayEquals(ByteArray(3), bytes)
            assertTrue(holder.hosts.value.isEmpty())
            assertEquals(listOf("disconnect", "close"), port.events)
        } finally { release.countDown(); worker.close() }
    }

    @Test
    fun dismissAfterHandlePublicationBeforeConnectResumesDoesNotRetireDestroyedHandle() = runTest {
        val main = StandardTestDispatcher(testScheduler)
        val worker = object : CoroutineDispatcher() {
            val tasks = ArrayDeque<Runnable>()
            override fun dispatch(context: CoroutineContext, block: Runnable) { tasks.addLast(block) }
        }
        val port = FakePort()
        val bytes = byteArrayOf(8, 3, 7)
        val holder = holder(connector = { _, _ -> port }, main = main, worker = worker)
        val connecting = launch { holder.connect(host, bytes) }
        runCurrent()
        val current = holder.host(host.id)!!
        assertNull(current.mutablePort.value)
        worker.tasks.removeFirst().run() // Publishes the handle; the main continuation is queued, not resumed.
        assertSame(port, current.mutablePort.value)
        assertFalse(connecting.isCompleted)
        holder.dismissHost(host.id)
        assertTrue(port.destroyed)
        runCurrent()
        connecting.join()
        assertNull(holder.host(host.id))
        assertArrayEquals(ByteArray(3), bytes)
        assertEquals(listOf("disconnect", "close"), port.events)
        assertThrows(IllegalStateException::class.java) { port.disconnect() }
    }

    // --- one connection per host, one unlock for many hosts ---------------------------------

    @Test
    fun aLiveHostIsNeverConnectedTwiceAndEachHostHasItsOwnConnection() = runTest {
        val ports = mutableMapOf<String, FakePort>()
        val holder = holder(connector = { request, _ -> FakePort().also { ports[request.addresses[0].host] = it } })
        val other = testHost(id = 8, label = "Other", addresses = listOf(HostEndpoint("other.invalid", 22)))
        holder.connect(host, byteArrayOf(1))
        holder.connect(host, byteArrayOf(2)) // Already live: skipped, and the array still wiped.
        holder.connect(other, byteArrayOf(3))
        assertEquals(setOf("fixture.invalid", "other.invalid"), ports.keys)
        assertEquals(setOf(7L, 8L), holder.hosts.value.keys)
        assertTrue(ports.values.none { it.destroyed })
        holder.disconnect(7)
        assertFalse(holder.isLive(7)) // A disconnecting connection is no longer live; the other is.
        assertTrue(holder.isLive(8))
        holder.dismissHost(7)
        holder.dismissHost(8)
    }

    @Test
    fun hostsSharingAKeyConnectFromOneArrayWipedOnlyAfterTheLastCall() = runTest {
        val bytes = byteArrayOf(5, 6, 7)
        val seen = mutableListOf<List<Byte>>()
        val holder = holder(connector = { request, _ ->
            seen += request.privateKey.toList() // Both calls must still see the key.
            FakePort()
        })
        val second = testHost(id = 8, label = "Second", addresses = listOf(HostEndpoint("two.invalid", 22)))
        holder.connect(listOf(host, second), bytes)
        assertEquals(listOf(listOf<Byte>(5, 6, 7), listOf<Byte>(5, 6, 7)), seen)
        assertArrayEquals(ByteArray(3), bytes)
        holder.dismissHost(7)
        holder.dismissHost(8)
    }

    @Test
    fun oneHostFailingDoesNotStopTheOthersAndTheFirstErrorIsRethrownAfterWiping() = runTest {
        val bytes = byteArrayOf(5, 6, 7)
        var calls = 0
        val holder = holder(connector = { request, _ ->
            calls++
            if (request.addresses[0].host == "fixture.invalid") throw HostConnectException.InvalidAddress(0u)
            FakePort()
        })
        val second = testHost(id = 8, label = "Second", addresses = listOf(HostEndpoint("two.invalid", 22)))
        try { holder.connect(listOf(host, second), bytes); fail("Expected the first failure") }
        catch (_: HostConnectException.InvalidAddress) { }
        assertEquals(2, calls)
        assertArrayEquals(ByteArray(3), bytes)
        assertEquals(setOf(8L), holder.hosts.value.keys)
        holder.dismissHost(8)
    }

    // --- terminals: any number per connection, M1's lifecycle each ----------------------------

    @Test
    fun severalTerminalsShareOneConnectionAndEndIndependently() = runTest {
        val port = FakePort()
        val holder = holder(connector = { _, _ -> port })
        holder.connect(host, byteArrayOf(1))
        connected(port)
        val active = holder.host(host.id)!!
        val shell = holder.openTerminal(active, TerminalTarget.Shell)
        val tmux = holder.openTerminal(active, TerminalTarget.Tmux("work"))
        val herdr = holder.openTerminal(active, TerminalTarget.Herdr(null, "w1:p2"))
        assertEquals(3, port.terminals.size)
        assertEquals(listOf(TerminalTarget.Shell, TerminalTarget.Tmux("work"), TerminalTarget.Herdr(null, "w1:p2")), port.terminals.map { it.first })
        assertEquals(listOf(shell, tmux, herdr), holder.terminals.value)
        assertEquals(listOf(1L, 2L, 3L), holder.terminals.value.map { it.id })
        assertEquals("shell", shell.title)
        assertEquals("tmux work", tmux.title)
        assertEquals("herdr w1:p2", herdr.title)
        assertSame(port.terminals[1].third, tmux.handle.value)

        port.terminals[1].second.onStateChanged(SessionState.Connected)
        port.terminals[1].second.onFrameReady()
        runCurrent()
        assertTrue(tmux.hasConnected.value)
        assertFalse(shell.hasConnected.value) // One session's callbacks never reach another.
        assertEquals(listOf(Unit), tmux.frameReady.replayCache)
        assertTrue(shell.frameReady.replayCache.isEmpty())

        holder.disconnectTerminal(tmux)
        assertEquals(listOf("disconnect"), port.terminals[1].third.events)
        assertTrue(port.terminals[0].third.events.isEmpty())
        port.terminals[1].second.onStateChanged(SessionState.Closed(CloseReason.Disconnected))
        runCurrent()
        assertEquals(SessionState.Closed(CloseReason.Disconnected), tmux.state.value)
        assertEquals(SessionState.Connecting, shell.state.value)
        holder.dismissTerminal(tmux)
        assertEquals(listOf(shell, herdr), holder.terminals.value)
        assertEquals(1, port.terminals[1].third.events.count { it == "close" })
        assertFalse(port.terminals[0].third.destroyed)
        holder.dismissHost(host.id)
    }

    @Test
    fun terminalsCannotOpenUntilConnectedOrAfterTheConnectionIsRetired() = runTest {
        val port = FakePort().apply { openFailure = HostException.NotConnected() }
        val holder = holder(connector = { _, _ -> port })
        holder.connect(host, byteArrayOf(1))
        val active = holder.host(host.id)!!
        assertThrows(HostException.NotConnected::class.java) { holder.openTerminal(active, TerminalTarget.Shell) }
        assertTrue(holder.terminals.value.isEmpty()) // A rejected open leaves nothing behind.
        holder.dismissHost(host.id)
        assertThrows(HostException.Closed::class.java) { holder.openTerminal(active, TerminalTarget.Shell) }
    }

    @Test
    fun hostClosureKeepsItsTerminalsReadableUntilDismissed() = runTest {
        val port = FakePort()
        lateinit var listener: HostListener
        val holder = holder(connector = { _, l -> listener = l; port })
        holder.connect(host, byteArrayOf(1))
        connected(port)
        val terminal = holder.openTerminal(holder.host(host.id)!!, TerminalTarget.Shell)
        holder.attachDisplay(terminal)
        holder.disconnect(host.id)
        assertEquals(listOf("disconnect"), port.events)
        port.terminals[0].second.onStateChanged(SessionState.Closed(CloseReason.Disconnected))
        listener.onHostStateChanged(HostState.Closed(CloseReason.Disconnected))
        runCurrent()
        assertEquals(HostState.Closed(CloseReason.Disconnected), holder.host(host.id)!!.state.value)
        assertEquals(SessionState.Closed(CloseReason.Disconnected), terminal.state.value)
        assertSame(port.terminals[0].third.lastFrame, terminal.handle.value!!.takeFrame()) // Final frame survives.
        holder.dismissHost(host.id) // Forgetting the connection does not touch its terminals.
        assertFalse(port.terminals[0].third.destroyed)
        assertEquals(listOf(terminal), holder.terminals.value)
        holder.detachDisplay(terminal)
        runCurrent()
        assertFalse(port.terminals[0].third.destroyed) // Still listed, not retired.
        holder.dismissTerminal(terminal)
        assertTrue(port.terminals[0].third.destroyed)
    }

    @Test
    fun releasingAHostCanDismissItsTerminalsToo() = runTest {
        val ports = mutableListOf<FakePort>()
        val holder = holder(connector = { _, _ -> FakePort().also { ports += it } })
        holder.connect(host, byteArrayOf(1))
        connected(ports[0])
        val active = holder.host(host.id)!!
        holder.openTerminal(active, TerminalTarget.Shell)
        holder.release(host.id, closeTerminals = false)
        assertEquals(1, holder.terminals.value.size)
        assertNull(holder.host(host.id))
        assertEquals(1, ports[0].events.count { it == "close" })
        holder.connect(host, byteArrayOf(2))
        holder.release(host.id, closeTerminals = true)
        assertTrue(holder.terminals.value.isEmpty())
        assertTrue(ports[0].terminals.all { it.third.destroyed })
    }

    @Test
    fun disconnectClosedLastFrameDismissThenDisposeClosesExactlyOnce() = runTest {
        val port = FakePort()
        val holder = holder(connector = { _, _ -> port })
        holder.connect(host, byteArrayOf(6, 3))
        connected(port)
        val displayed = holder.openTerminal(holder.host(host.id)!!, TerminalTarget.Shell)
        val session = port.terminals[0].third
        holder.attachDisplay(displayed)
        holder.disconnectTerminal(displayed)
        port.terminals[0].second.onStateChanged(SessionState.Closed(CloseReason.Disconnected))
        runCurrent()
        assertEquals(listOf(displayed), holder.terminals.value)
        assertEquals(SessionState.Closed(CloseReason.Disconnected), displayed.state.value)
        assertSame(session.lastFrame, displayed.handle.value!!.takeFrame())
        holder.dismissTerminal(displayed)
        assertTrue(holder.terminals.value.isEmpty())
        assertFalse(session.destroyed) // Still in composition; a queued draw must not crash.
        holder.detachDisplay(displayed)
        assertFalse(session.destroyed) // Terminal child disposal finishes on this main-loop turn.
        runCurrent()
        assertEquals(1, session.events.count { it == "close" })
        holder.dismissTerminal(displayed)
        runCurrent()
        assertEquals(1, session.events.count { it == "close" })
        assertThrows(IllegalStateException::class.java) { session.takeFrame() }
        holder.dismissHost(host.id)
    }

    @Test
    fun recreationRetainsTheTerminalAndDismissalWaitsForDisplayDisposal() = runTest {
        val port = FakePort()
        val holder = holder(connector = { _, _ -> port })
        holder.connect(host, byteArrayOf(7))
        connected(port)
        val terminal = holder.openTerminal(holder.host(host.id)!!, TerminalTarget.Shell)
        val session = port.terminals[0].third
        holder.attachDisplay(terminal)
        holder.detachDisplay(terminal) // Activity recreation or navigation away, not dismissal.
        runCurrent()
        assertFalse(session.destroyed)
        holder.attachDisplay(terminal)
        holder.dismissTerminal(terminal)
        assertFalse(session.destroyed)
        assertSame(session.lastFrame, terminal.handle.value!!.takeFrame())
        holder.detachDisplay(terminal)
        runCurrent()
        assertEquals(1, session.events.count { it == "close" })
        holder.dismissHost(host.id)
    }

    // --- capabilities and herdr watches -------------------------------------------------------

    @Test
    fun connectedHostProbesCapabilitiesAndWatchesEachRunningHerdrSessionOnce() = runTest {
        val port = FakePort().apply {
            caps = HostCapabilities("/usr/bin/tmux", "/home/x/.local/bin/herdr", null, "C.UTF-8", listOf(
                HerdrSessionInfo("default", true, true), HerdrSessionInfo("work", true, false),
                HerdrSessionInfo("idle", false, false),
            ))
        }
        lateinit var listener: HostListener
        val holder = holder(connector = { _, l -> listener = l; port })
        holder.connect(host, byteArrayOf(1))
        val active = holder.host(host.id)!!
        listener.onHostStateChanged(HostState.Connected(0u))
        runCurrent()
        assertEquals(port.caps, active.capabilities.value)
        // The default session is watched with a null name (never its listed name); stopped ones not at all.
        assertEquals(listOf<String?>(null, "work"), port.watches.map { it.first })
        assertEquals(listOf("default", "work"), active.watches.value.map { it.name })

        val view = HerdrView(1uL, 22u, null, emptyList(), emptyList(), emptyList(), emptyList())
        port.watches[1].second.onHerdrStateChanged(HerdrState.Live(view))
        runCurrent()
        assertEquals(HerdrState.Live(view), active.watches.value[1].state.value)
        assertEquals(HerdrState.Starting, active.watches.value[0].state.value)

        // Refresh: a new session starts, one vanishes; existing watches are kept.
        port.caps = port.caps.copy(herdrSessions = listOf(
            HerdrSessionInfo("default", true, true), HerdrSessionInfo("fresh", true, false)))
        holder.refresh(active)
        assertEquals(listOf<String?>(null, "work", "fresh"), port.watches.map { it.first })
        assertEquals(listOf("default", "fresh"), active.watches.value.map { it.name })
        assertEquals(1, port.watches[1].third.stops)
        assertEquals(1, port.watches[1].third.closes)
        assertEquals(0, port.watches[0].third.stops)

        holder.dismissHost(host.id) // Retiring stops and releases every watch before the connection closes.
        assertTrue(port.watches.all { it.third.stops == 1 && it.third.closes == 1 })
        assertTrue(active.watches.value.isEmpty())
    }

    @Test
    fun noHerdrOrAFailedProbeStartsNoWatchesAndExplainsWhy() = runTest {
        val port = FakePort().apply { caps = caps.copy(herdr = null) }
        lateinit var listener: HostListener
        val holder = holder(connector = { _, l -> listener = l; port })
        holder.connect(host, byteArrayOf(1))
        val active = holder.host(host.id)!!
        listener.onHostStateChanged(HostState.Connected(0u))
        runCurrent()
        assertTrue(port.watches.isEmpty())
        assertNotNull(active.capabilities.value)
        assertNull(active.capabilitiesError.value)

        port.capsFailure = HostException.CommandFailed("probe")
        port.caps = port.caps.copy(herdr = "/x/herdr")
        holder.refresh(active)
        assertNotNull(active.capabilitiesError.value)
        assertTrue(port.watches.isEmpty())
        port.capsFailure = null
        holder.refresh(active)
        assertNull(active.capabilitiesError.value)
        assertEquals(1, port.watches.size)
        holder.dismissHost(host.id)
    }

    @Test
    fun aFailingWatchOpenShowsAsUnavailableWithoutAffectingTheHost() = runTest {
        val port = FakePort().apply { watchFailure = HostException.InvalidName() }
        lateinit var listener: HostListener
        val holder = holder(connector = { _, l -> listener = l; port })
        holder.connect(host, byteArrayOf(1))
        listener.onHostStateChanged(HostState.Connected(0u))
        runCurrent()
        val watch = holder.host(host.id)!!.watches.value.single()
        assertTrue(watch.state.value is HerdrState.Unavailable)
        assertTrue(holder.isLive(host.id))
        holder.dismissHost(host.id)
    }

    @Test
    fun tmuxListingGoesThroughTheConnection() = runTest {
        val port = FakePort().apply { tmux = listOf(TmuxSession("main", 3u, 1u, 100L, 200L)) }
        val holder = holder(connector = { _, _ -> port })
        holder.connect(host, byteArrayOf(1))
        assertEquals(listOf("main"), holder.listTmuxSessions(holder.host(host.id)!!).map { it.name })
        holder.dismissHost(host.id)
    }
}
