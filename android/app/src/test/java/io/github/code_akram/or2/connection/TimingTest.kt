package io.github.code_akram.or2.connection

import io.github.code_akram.or2.ffi.*
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import org.junit.Assert.*
import org.junit.Test

/** The `or2.timing` markers: what a span logs, and the connect, capability and first-view marks of the holder. */
@OptIn(ExperimentalCoroutinesApi::class)
class TimingTest {
    @Test
    fun aSpanLogsEachEventWithTheMillisecondsSinceItBegan() {
        val lines = mutableListOf<String>()
        var clock = 10_000L
        val timing = Timing(lines::add) { clock }
        timing.begin("connect host=3", "unlocked")
        clock += 120
        timing.mark("connect host=3", "connected")
        clock += 30
        timing.mark("connect host=3", "live")
        clock += 5
        timing.end("connect host=3", "done")
        assertEquals(
            listOf("connect host=3 unlocked ms=0", "connect host=3 connected ms=120", "connect host=3 live ms=150", "connect host=3 done ms=155"),
            lines,
        )
        // Over: nothing more is logged for it, and a span that never began logs nothing.
        timing.mark("connect host=3", "late")
        timing.mark("never", "x")
        assertEquals(4, lines.size)
        assertFalse(timing.isRunning("connect host=3"))
    }

    @Test
    fun withoutASinkNothingIsRecordedAndNothingFails() {
        val timing = Timing()
        assertFalse(timing.enabled)
        timing.begin("a")
        timing.mark("a", "b")
        timing.watchTerminal(1, "a")
        timing.terminalConnected(1)
        timing.terminalFrame(1)
        timing.end("a", "c")
        assertFalse(timing.isRunning("a"))
    }

    @Test
    fun aWatchedTerminalReportsItsConnectAndItsFirstFrameOnce() {
        val lines = mutableListOf<String>()
        var clock = 0L
        val timing = Timing(lines::add) { clock }
        timing.begin("tap host=1 pane=w1:p1")
        timing.watchTerminal(9, "tap host=1 pane=w1:p1")
        clock += 40
        timing.terminalConnected(9)
        clock += 10
        timing.terminalFrame(9)
        timing.terminalFrame(9)
        timing.terminalConnected(9)
        assertEquals(
            listOf("tap host=1 pane=w1:p1 begin ms=0", "tap host=1 pane=w1:p1 terminal-connected ms=40", "tap host=1 pane=w1:p1 frame ms=50"),
            lines,
        )
        // A terminal that closes before its first frame is forgotten.
        timing.begin("reopen host=1")
        timing.watchTerminal(10, "reopen host=1")
        timing.forgetTerminal(10)
        timing.terminalFrame(10)
        assertEquals(4, lines.size)
    }

    @Test
    fun aRetainedTerminalIsTimedAgainByEachActivationAndUnwatchedFramesAreIgnored() {
        val lines = mutableListOf<String>()
        var clock = 0L
        val timing = Timing(lines::add) { clock }
        timing.terminalFrame(5) // Frames drawn while nobody waits (the view reports every one).
        timing.begin("reuse host=1 pane=p")
        timing.watchTerminal(5, "reuse host=1 pane=p")
        clock += 30
        timing.terminalFrame(5)
        // The app returns from the background: the same terminal is activated again.
        clock += 1000
        timing.begin("reopen host=1")
        timing.watchTerminal(5, "reopen host=1")
        clock += 20
        timing.terminalFrame(5)
        timing.terminalFrame(5)
        assertEquals(
            listOf("reuse host=1 pane=p begin ms=0", "reuse host=1 pane=p frame ms=30", "reopen host=1 begin ms=0", "reopen host=1 frame ms=20"),
            lines,
        )
    }

    private fun TestScope.holder(timing: Timing, port: FakePort, listener: (HostListener) -> Unit) =
        HostConnections({ _, l -> listener(l); port }, FakeTrust(), StandardTestDispatcher(testScheduler), UnconfinedTestDispatcher(testScheduler), timing = timing)

    @Test
    fun theConnectPathMarksUnlockedConnectedCapabilitiesMoshServerAndTheFirstLiveViewOnce() = runTest {
        val lines = mutableListOf<String>()
        var clock = 500L
        lateinit var hostListener: HostListener
        val port = FakePort()
        val holder = holder(Timing(lines::add) { clock }, port) { hostListener = it }
        holder.connect(testHost(), byteArrayOf(1)) // The biometric is done: this is where the clock starts.
        advanceUntilIdle()
        clock += 100
        port.nativeState = HostState.Authenticating
        hostListener.onHostStateChanged(HostState.Authenticating)
        advanceUntilIdle()
        clock += 250
        port.nativeState = HostState.Connected(0u)
        hostListener.onHostStateChanged(HostState.Connected(0u))
        advanceUntilIdle() // The probe answers, and the default session is watched.
        clock += 400
        val view = HerdrView(1uL, null, emptyList(), emptyList(), emptyList(), emptyList())
        port.watches[0].second.onHerdrStateChanged(HerdrState.Live(view))
        advanceUntilIdle()
        // Later views of the same host are not the first.
        port.watches[0].second.onHerdrStateChanged(HerdrState.Live(HerdrView(2uL, null, emptyList(), emptyList(), emptyList(), emptyList())))
        advanceUntilIdle()
        assertEquals(
            listOf(
                "connect host=7 unlocked ms=0",
                "connect host=7 authenticating ms=100",
                "connect host=7 connected ms=350",
                "connect host=7 capabilities ms=350",
                "connect host=7 mosh-server ms=350",
                "connect host=7 live ms=750",
            ),
            lines,
        )
        holder.dismissHost(7)
    }

    @Test
    fun aHostThatClosesBeforeItConnectedEndsItsConnectSpanAsFailed() = runTest {
        val lines = mutableListOf<String>()
        var clock = 0L
        lateinit var hostListener: HostListener
        val holder = holder(Timing(lines::add) { clock }, FakePort()) { hostListener = it }
        holder.connect(testHost(id = 4), byteArrayOf(1))
        advanceUntilIdle()
        clock += 20_000 // the unreachable host's 20 s
        hostListener.onHostStateChanged(HostState.Closed(CloseReason.Failed(SessionFailure.TimedOut)))
        advanceUntilIdle()
        assertEquals(listOf("connect host=4 unlocked ms=0", "connect host=4 failed ms=20000"), lines)
        // A host that connected and later closed is not a failure of the connect.
        holder.dismissHost(4)
    }

    @Test
    fun aTapThatOpensOverSshIsTimedToItsFirstFrameAndTheSwapMarksTheVerdictNotASecondConnect() = runTest {
        val lines = mutableListOf<String>()
        var clock = 0L
        lateinit var hostListener: HostListener
        val port = FakePort().apply { caps = caps.copy(moshServer = "/usr/bin/mosh-server") }
        val holder = holder(Timing(lines::add) { clock }, port) { hostListener = it }
        holder.connect(testHost(), byteArrayOf(1))
        port.nativeState = HostState.Connected(0u)
        hostListener.onHostStateChanged(HostState.Connected(0u))
        advanceUntilIdle()
        lines.clear()

        holder.timing.begin("tap host=7 pane=w1:p1")
        val terminal = holder.openTerminal(holder.host(7)!!, TerminalTarget.Herdr(null, "w1:p1"))
        holder.timing.watchTerminal(terminal.id, "tap host=7 pane=w1:p1")
        clock += 40
        port.terminals[0].second.onStateChanged(SessionState.Connected) // SSH: no wait for UDP.
        advanceUntilIdle()
        holder.timing.terminalFrame(terminal.id)
        clock += 300
        port.terminals[1].second.onStateChanged(SessionState.Connected) // The background mosh session.
        advanceUntilIdle()
        assertEquals(
            listOf(
                "tap host=7 pane=w1:p1 begin ms=0",
                "tap host=7 pane=w1:p1 terminal-connected ms=40",
                "tap host=7 pane=w1:p1 frame ms=40",
                "connect host=7 udp-ok ms=340",
            ),
            lines,
        )
        holder.dismissHost(7)
    }
}
