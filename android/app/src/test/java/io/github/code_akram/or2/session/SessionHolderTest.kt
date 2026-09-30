package io.github.code_akram.or2.session

import io.github.code_akram.or2.data.HostRecord
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
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.*
import org.junit.Test
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import kotlin.coroutines.CoroutineContext

@OptIn(ExperimentalCoroutinesApi::class)
class SessionHolderTest {
    private val host = HostRecord(7, "Fixture", "fixture.invalid", 2222, "fixture", "ephemeral")
    private val public = PublicKeyInfo("test-algorithm", "test-public-line", "test-fingerprint", "")
    private val prompt = SessionState.AwaitingHostKeyDecision(public, emptyList())

    private class Store(val events: MutableList<String> = mutableListOf()) : TrustStore {
        var lines = listOf("prior-line-one", "prior-line-two")
        var fail = false
        var persistedHost: HostRecord? = null
        var gate: CompletableDeferred<Unit>? = null
        var replacements = 0
        override suspend fun trustedKeys(hostId: Long) = lines
        override suspend fun replaceTrust(host: HostRecord, presented: PublicKeyInfo) {
            replacements++
            gate?.await()
            if (fail) error("storage failure")
            persistedHost = host
            lines = listOf(presented.openssh)
            events += "persist"
        }
    }

    private class FakeSession(val events: MutableList<String> = mutableListOf()) : SessionInterface, AutoCloseable {
        var approved: String? = null
        var takes = 0
        var destroyed = false
        var nativeState: SessionState = SessionState.Connecting
        val lastFrame = TerminalFrame(1uL, 2u, 1u, true,
            listOf(CellStyle(0xffffffu, 0u, null, Underline.NONE, false, false, false, false, false)),
            listOf(TerminalRow(0u, false, listOf(TerminalCell("L", CellWidth.NARROW, 0u), TerminalCell("R", CellWidth.NARROW, 0u)))),
            null, 0u, Scrollback(1uL, 0uL))
        var pending: TerminalFrame? = lastFrame
        override fun approveHostKey(fingerprint: String) { approved = fingerprint; events += "approve" }
        override fun rejectHostKey() { events += "reject" }
        override fun disconnect() {
            check(!destroyed) { "Session object has already been destroyed" }
            events += "disconnect"
        }
        override fun close() { destroyed = true; events += "close" }
        override fun requestFullFrame() = Unit
        override fun resize(columns: UShort, rows: UShort) = Unit
        override fun scroll(scroll: ViewportScroll) = Unit
        override fun sendKey(input: KeyInput) = Unit
        override fun sendText(text: String) = Unit
        override fun state() = nativeState
        override fun takeFrame(): TerminalFrame? {
            check(!destroyed) { "Session object has already been destroyed" }
            takes++
            return pending.also { pending = null }
        }
    }

    @Test
    fun earlyCallbacksAndApprovalWaitForAssignmentAndNeverTakeFrames() = runTest {
        val main = StandardTestDispatcher(testScheduler)
        val worker = UnconfinedTestDispatcher(testScheduler)
        val store = Store()
        val fake = FakeSession(store.events)
        val bytes = byteArrayOf(9, 2, 5)
        lateinit var holder: SessionHolder
        lateinit var approval: Job
        holder = SessionHolder(SessionConnector { request, listener ->
            assertArrayEquals(byteArrayOf(9, 2, 5), request.privateKey) // Not wiped at construction.
            assertEquals(listOf("prior-line-one", "prior-line-two"), request.trustedHostKeys)
            assertEquals(2222.toUShort(), request.port)
            fake.nativeState = prompt
            listener.onStateChanged(prompt)
            listener.onFrameReady()
            testScheduler.runCurrent() // Main processes callbacks while factory still hasn't returned.
            val current = holder.active.value!!
            assertNull(current.handle.value)
            assertEquals(prompt, current.state.value)
            approval = launch(start = CoroutineStart.UNDISPATCHED) { holder.approve(current, prompt) }
            assertTrue(store.events.isEmpty())
            fake
        }, store, main, worker)
        holder.connect(host, bytes)
        approval.join()
        val current = holder.active.value!!
        assertSame(fake, current.handle.value)
        assertEquals(listOf(Unit), current.frameReady.replayCache)
        assertEquals(listOf("persist", "approve"), store.events)
        assertEquals(public.fingerprint, fake.approved)
        assertEquals(host, store.persistedHost)
        assertEquals(listOf(public.openssh), store.lines)
        assertArrayEquals(ByteArray(3), bytes)
        assertEquals(0, fake.takes)
        holder.disconnect()
        assertFalse(fake.destroyed)
        holder.dismiss()
        assertEquals(1, store.events.count { it == "close" })
    }

    @Test
    fun persistenceFailureAndExpiredPromptNeverApprove() = runTest {
        val dispatcher = UnconfinedTestDispatcher(testScheduler)
        val store = Store().apply { fail = true }
        val fake = FakeSession()
        lateinit var listener: SessionListener
        val holder = SessionHolder(SessionConnector { _, incoming -> listener = incoming; fake }, store, dispatcher, dispatcher)
        holder.connect(host, byteArrayOf(3, 8))
        fake.nativeState = prompt
        listener.onStateChanged(prompt)
        val current = holder.active.value!!
        try { holder.approve(current, prompt); fail("Expected storage failure") } catch (_: IllegalStateException) { }
        assertNull(fake.approved)
        assertEquals(listOf("prior-line-one", "prior-line-two"), store.lines)
        store.fail = false
        listener.onStateChanged(SessionState.Closed(CloseReason.Failed(SessionFailure.TimedOut)))
        try { holder.approve(current, prompt); fail("Expected expired prompt") } catch (_: IllegalStateException) { }
        assertNull(fake.approved)
        holder.disconnect()
    }

    @Test
    fun nativeClosedBeforeQueuedCallbackNeverStartsTrustPersistence() = runTest {
        val main = StandardTestDispatcher(testScheduler)
        val worker = UnconfinedTestDispatcher(testScheduler)
        val store = Store()
        val fake = FakeSession()
        lateinit var listener: SessionListener
        val holder = SessionHolder(SessionConnector { _, incoming -> listener = incoming; fake }, store, main, worker)
        holder.connect(host, byteArrayOf(4, 2))
        fake.nativeState = prompt
        listener.onStateChanged(prompt)
        runCurrent()
        val current = holder.active.value!!
        val closed = SessionState.Closed(CloseReason.Failed(SessionFailure.ConnectionLost("Fixture closed")))
        fake.nativeState = closed
        listener.onStateChanged(closed) // Main delivery remains queued while Trust is tapped.
        assertEquals(prompt, current.state.value)
        assertEquals(closed, current.handle.value!!.state())
        val failure = runCatching { holder.approve(current, prompt) }.exceptionOrNull()
        assertEquals(0, store.replacements)
        assertEquals(listOf("prior-line-one", "prior-line-two"), store.lines)
        assertNull(fake.approved)
        assertTrue(failure is IllegalStateException)
        assertEquals("Host-key prompt has expired.", failure!!.message)
        runCurrent()
        assertEquals(closed, current.state.value)
        holder.dismiss()
    }

    @Test
    fun closeDuringPersistenceAndReplacedSessionCallbacksAreIsolated() = runTest {
        val dispatcher = UnconfinedTestDispatcher(testScheduler)
        val store = Store().apply { gate = CompletableDeferred() }
        val fake = FakeSession()
        val listeners = mutableListOf<SessionListener>()
        val holder = SessionHolder(SessionConnector { _, listener ->
            listeners += listener
            if (listeners.size == 1) fake else FakeSession()
        }, store, dispatcher, dispatcher)
        holder.connect(host, byteArrayOf(6))
        fake.nativeState = prompt
        listeners[0].onStateChanged(prompt)
        val previous = holder.active.value!!
        val approval = launch(start = CoroutineStart.UNDISPATCHED) { holder.approve(previous, prompt) }
        assertEquals(1, store.replacements) // Write began while both native and cached prompt were live.
        fake.nativeState = SessionState.Closed(CloseReason.Failed(SessionFailure.ConnectionLost("Fixture closed")))
        listeners[0].onStateChanged(fake.nativeState)
        store.gate!!.complete(Unit)
        approval.join()
        assertEquals(host, store.persistedHost)
        assertEquals(listOf(public.openssh), store.lines) // An already-started write still completes.
        assertEquals(listOf("persist"), store.events)
        assertNull(fake.approved)
        holder.connect(host.copy(id = 8), byteArrayOf(7))
        listeners[0].onStateChanged(SessionState.Connected)
        listeners[0].onFrameReady()
        assertEquals(SessionState.Connecting, holder.active.value!!.state.value)
        assertTrue(holder.active.value!!.frameReady.replayCache.isEmpty())
        holder.reject(holder.active.value!!)
        assertTrue((holder.active.value!!.handle.value as FakeSession).events.contains("reject"))
        holder.dismiss()
    }

    @Test
    fun synchronousFactoryAndTrustReadFailuresStillWipe() = runTest {
        val dispatcher = UnconfinedTestDispatcher(testScheduler)
        val bytes = byteArrayOf(11, 4, 1)
        val holder = SessionHolder(SessionConnector { request, _ ->
            assertArrayEquals(byteArrayOf(11, 4, 1), request.privateKey)
            throw ConnectException.InvalidPrivateKey()
        }, Store(), dispatcher, dispatcher)
        try { holder.connect(host, bytes); fail("Expected validation error") } catch (_: ConnectException.InvalidPrivateKey) { }
        assertArrayEquals(ByteArray(3), bytes)
        assertNull(holder.active.value)
        val readFailure = object : TrustStore {
            override suspend fun trustedKeys(hostId: Long): List<String> = error("read failed")
            override suspend fun replaceTrust(host: HostRecord, presented: PublicKeyInfo) = Unit
        }
        val other = SessionHolder(SessionConnector { _, _ -> fail("Must not call factory"); FakeSession() }, readFailure, dispatcher, dispatcher)
        val secret = byteArrayOf(3, 5)
        try { other.connect(host, secret); fail("Expected read failure") } catch (_: IllegalStateException) { }
        assertArrayEquals(ByteArray(2), secret)
    }

    @Test
    fun cancellationAtFactoryReturnWipesButDoesNotLeakTheNativeHandle() = runTest {
        val worker = Executors.newSingleThreadExecutor().asCoroutineDispatcher()
        val main = StandardTestDispatcher(testScheduler)
        val entered = CompletableDeferred<Unit>()
        val release = CountDownLatch(1)
        val fake = FakeSession()
        val bytes = byteArrayOf(4, 9, 2)
        val holder = SessionHolder(SessionConnector { request, _ ->
            entered.complete(Unit)
            check(release.await(5, TimeUnit.SECONDS))
            assertArrayEquals(byteArrayOf(4, 9, 2), request.privateKey)
            fake
        }, Store(), main, worker)
        try {
            val job = launch { holder.connect(host, bytes) }
            runCurrent()
            entered.await()
            job.cancel()
            assertArrayEquals(byteArrayOf(4, 9, 2), bytes) // Factory still needs these bytes.
            release.countDown()
            job.join()
            assertArrayEquals(ByteArray(3), bytes)
            assertNull(holder.active.value)
            assertEquals(listOf("disconnect", "close"), fake.events)
        } finally { release.countDown(); worker.close() }
    }

    @Test
    fun disconnectClosedLastFrameDismissThenDisposeClosesExactlyOnce() = runTest {
        val main = StandardTestDispatcher(testScheduler)
        val worker = UnconfinedTestDispatcher(testScheduler)
        val fake = FakeSession()
        lateinit var listener: SessionListener
        val holder = SessionHolder(SessionConnector { _, incoming -> listener = incoming; fake }, Store(), main, worker)
        holder.connect(host, byteArrayOf(6, 3))
        val displayed = holder.active.value!!
        holder.attachDisplay(displayed)
        holder.disconnect()
        listener.onStateChanged(SessionState.Closed(CloseReason.Disconnected))
        runCurrent()
        assertSame(displayed, holder.active.value)
        assertEquals(SessionState.Closed(CloseReason.Disconnected), displayed.state.value)
        assertSame(fake, displayed.handle.value)
        assertSame(fake.lastFrame, displayed.handle.value!!.takeFrame())
        holder.dismiss()
        assertNull(holder.active.value)
        assertFalse(fake.destroyed) // Still in composition; a queued draw must not crash.
        holder.detachDisplay(displayed)
        assertFalse(fake.destroyed) // Terminal child disposal finishes on this main-loop turn.
        runCurrent()
        assertEquals(1, fake.events.count { it == "close" })
        holder.dismiss()
        runCurrent()
        assertEquals(1, fake.events.count { it == "close" })
        assertThrows(IllegalStateException::class.java) { fake.takeFrame() }
    }

    @Test
    fun recreationRetainsHandleAndReplacementWaitsForOldDisplayDisposal() = runTest {
        val main = StandardTestDispatcher(testScheduler)
        val worker = UnconfinedTestDispatcher(testScheduler)
        val sessions = mutableListOf<FakeSession>()
        val holder = SessionHolder(SessionConnector { _, _ -> FakeSession().also { sessions += it } }, Store(), main, worker)
        holder.connect(host, byteArrayOf(7))
        val previous = holder.active.value!!
        holder.attachDisplay(previous)
        holder.detachDisplay(previous) // Activity recreation or navigation away, not dismissal.
        runCurrent()
        assertFalse(sessions[0].destroyed)
        holder.attachDisplay(previous)
        holder.connect(host.copy(id = 8), byteArrayOf(8))
        assertFalse(sessions[0].destroyed)
        assertSame(sessions[0].lastFrame, previous.handle.value!!.takeFrame())
        holder.detachDisplay(previous)
        runCurrent()
        assertEquals(1, sessions[0].events.count { it == "close" })
        holder.dismiss()
        assertEquals(1, sessions[1].events.count { it == "close" })
    }

    @Test
    fun dismissAfterHandlePublicationBeforeConnectResumesDoesNotRetireDestroyedHandle() = runTest {
        val main = StandardTestDispatcher(testScheduler)
        val worker = object : CoroutineDispatcher() {
            val tasks = ArrayDeque<Runnable>()
            override fun dispatch(context: CoroutineContext, block: Runnable) { tasks.addLast(block) }
        }
        val fake = FakeSession()
        val bytes = byteArrayOf(8, 3, 7)
        val holder = SessionHolder(SessionConnector { _, _ -> fake }, Store(), main, worker)
        val connecting = launch { holder.connect(host, bytes) }
        runCurrent()
        val current = holder.active.value!!
        assertNull(current.handle.value)
        worker.tasks.removeFirst().run() // Publishes handle; main continuation is queued, not resumed.
        assertSame(fake, current.handle.value)
        assertFalse(connecting.isCompleted)
        holder.dismiss()
        assertTrue(fake.destroyed)
        runCurrent()
        connecting.join()
        assertNull(holder.active.value)
        assertArrayEquals(ByteArray(3), bytes)
        assertEquals(listOf("disconnect", "close"), fake.events)
        assertThrows(IllegalStateException::class.java) { fake.disconnect() }
    }
}
