package io.github.code_akram.or2

import io.github.code_akram.or2.ffi.LinkHealth
import io.github.code_akram.or2.ffi.SessionListener
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.Session
import io.github.code_akram.or2.ffi.TerminalFrame
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.fail

/**
 * Records callbacks for assertions on the test thread. JUnit assertion errors thrown inside a
 * callback would not reach the test, so callbacks only record.
 */
class RecordingListener(private val throwAfterRecording: Boolean = false) : SessionListener {
    val states = LinkedBlockingQueue<SessionState>()
    private val frameReady = LinkedBlockingQueue<Unit>()
    val healths = LinkedBlockingQueue<LinkHealth>()
    val clipboardWrites = LinkedBlockingQueue<String>()
    val serverPids = LinkedBlockingQueue<UInt>()
    val callbackThreads: MutableSet<Thread> = ConcurrentHashMap.newKeySet()
    private val active = AtomicInteger()

    @Volatile
    var overlapped = false
        private set

    /** Optional log shared across recorders: "tag:StateName", appended as each state arrives. */
    @Volatile
    var timeline: MutableList<String>? = null

    @Volatile
    var timelineTag = "session"

    override fun onStateChanged(state: SessionState) = record {
        timeline?.add("$timelineTag:${state::class.simpleName}")
        states.add(state)
    }

    override fun onFrameReady() = record { frameReady.add(Unit) }

    override fun onLinkHealth(health: LinkHealth) = record { healths.add(health) }

    override fun onClipboardWrite(text: String) = record { clipboardWrites.add(text) }

    /** `on_server_pid`, in order with the states (the timeline gets `tag:pid N`). */
    override fun onServerPid(pid: UInt) = record {
        timeline?.add("$timelineTag:pid $pid")
        serverPids.add(pid)
    }

    private fun record(action: () -> Unit) {
        if (active.incrementAndGet() != 1) overlapped = true
        callbackThreads.add(Thread.currentThread())
        action()
        active.decrementAndGet()
        if (throwAfterRecording) throw IllegalStateException("listener failure")
    }

    /** The next state change, which must be a [T]: this checks delivery order. */
    inline fun <reified T : SessionState> awaitState(): T {
        val state = states.poll(TIMEOUT_SECONDS, TimeUnit.SECONDS)
        assertNotNull("timed out waiting for ${T::class.simpleName}", state)
        if (state !is T) fail("expected ${T::class.simpleName}, got $state")
        return state as T
    }

    fun awaitHealth(): LinkHealth {
        val health = healths.poll(TIMEOUT_SECONDS, TimeUnit.SECONDS)
        assertNotNull("timed out waiting for link health", health)
        return health!!
    }

    fun assertNoMoreStates() {
        assertNull(states.poll(QUIET_MILLIS, TimeUnit.MILLISECONDS))
    }

    /** A live producer may publish again immediately after the take; only the probe is quiescent. */
    fun awaitFrame(session: Session, quiescent: Boolean = true): TerminalFrame {
        assertNotNull("timed out waiting for a frame", frameReady.poll(TIMEOUT_SECONDS, TimeUnit.SECONDS))
        val frame = session.takeFrame()
        assertNotNull("frame ready but nothing to take", frame)
        if (quiescent) assertNull("one take drains the mailbox", session.takeFrame())
        return frame!!
    }

    fun assertNoFrameSignal() {
        assertEquals(null, frameReady.poll(QUIET_MILLIS, TimeUnit.MILLISECONDS))
    }

    companion object {
        const val TIMEOUT_SECONDS = 5L
        const val QUIET_MILLIS = 200L
    }
}
