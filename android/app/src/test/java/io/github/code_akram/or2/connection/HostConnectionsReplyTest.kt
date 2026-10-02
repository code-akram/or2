package io.github.code_akram.or2.connection

import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.HostException
import io.github.code_akram.or2.ffi.HostListener
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.ReplyRoute
import io.github.code_akram.or2.ffi.SessionFailure
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** A notification's reply (`HostConnections.replyToPane`) on fakes: only a live, connected connection carries it. */
@OptIn(ExperimentalCoroutinesApi::class)
class HostConnectionsReplyTest {
    private val host = testHost()
    private val ports = mutableListOf<FakePort>()
    private val listeners = mutableListOf<HostListener>()
    private val lines = mutableListOf<String>()

    private fun TestScope.holder() = HostConnections(
        { _, listener -> listeners += listener; FakePort().also { ports += it } },
        FakeTrust(), StandardTestDispatcher(testScheduler), UnconfinedTestDispatcher(testScheduler),
        timing = Timing({ lines += it }),
    )

    private suspend fun failure(block: suspend () -> Unit): Throwable? = try {
        block()
        null
    } catch (e: HostException) {
        e
    }

    @Test
    fun aReplyGoesOverTheLiveConnectionOnlyAndNeverConnects() = runTest {
        val holder = holder()
        val reply = suspend { holder.replyToPane(host.id, "work", "w1:p2", "secret reply") }
        // No connection at all: not connected, and nothing is connected for it.
        assertTrue(failure { reply() } is HostException.NotConnected)
        assertTrue(ports.isEmpty())

        holder.connect(host, byteArrayOf(1))
        advanceUntilIdle()
        // Still connecting: not yet.
        assertTrue(failure { reply() } is HostException.NotConnected)
        assertTrue(ports.single().replies.isEmpty())

        ports.single().nativeState = HostState.Connected(0u)
        listeners.single().onHostStateChanged(HostState.Connected(0u))
        advanceUntilIdle()
        ports.single().replyRoute = ReplyRoute.TYPED
        assertEquals(ReplyRoute.TYPED, reply())
        assertEquals(listOf(Triple("work", "w1:p2", "secret reply")), ports.single().replies)
        // The reply's own failure is the caller's to show.
        ports.single().replyFailure = HostException.PaneNotFound()
        assertTrue(failure { reply() } is HostException.PaneNotFound)

        // Lost: not connected, and no reconnect from here.
        listeners.single().onHostStateChanged(HostState.Closed(CloseReason.Failed(SessionFailure.ConnectionLost("reset"))))
        advanceUntilIdle()
        assertTrue(failure { reply() } is HostException.NotConnected)
        assertEquals(1, ports.size)
        assertEquals(2, ports.single().replies.size)
        // Nothing a reply says reaches the timing log.
        assertTrue(lines.none { "secret" in it })
    }
}
