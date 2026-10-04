package io.github.code_akram.or2.app

import io.github.code_akram.or2.connection.ActiveTerminal
import io.github.code_akram.or2.connection.FakePort
import io.github.code_akram.or2.connection.Timing
import io.github.code_akram.or2.connection.UdpVerdict
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.TransportPref
import io.github.code_akram.or2.ffi.HostListener
import io.github.code_akram.or2.ffi.SessionFailure
import io.github.code_akram.or2.ffi.TerminalTransport
import io.github.code_akram.or2.connection.FakeTrust
import io.github.code_akram.or2.connection.HostConnections
import io.github.code_akram.or2.connection.testHost
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.HostException
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalTarget
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.*
import org.junit.Test

/**
 * Only an explicit agent request (an inbox row, a notification) awaits `focusHerdrPane` for its pane first, new or
 * reused terminal alike; a gone pane or a failed focus shows a message and never an activation. Everything else that
 * brings an open terminal back (a picker row marked `Open`, a reattach) shows it as it is. One herdr session has one
 * terminal per host: every open reuses it, whatever pane it was opened on.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class TerminalActivationsTest {
    private val host = testHost()
    private val a = TerminalTarget.Herdr(null, "w1:p1")
    private val b = TerminalTarget.Herdr(null, "w1:p2")

    private class Setup(val holder: HostConnections, val port: FakePort, val events: MutableList<String>, val hostListener: HostListener) {
        val activations get() = holder.activations
        fun open(target: TerminalTarget) = holder.openTerminal(holder.host(7)!!, target)
    }

    /**
     * A connected host. [pending] leaves the capability probe unanswered until `port.capsGate` completes,
     * [moshPending] `mosh_server()` until `port.moshServerGate` does. [udp] is the connection's verdict:
     * `OK` by default, so each terminal is one mosh session (the background attempt and the swap are
     * HostConnectionsTransportTest's); null leaves it `UNKNOWN`.
     */
    private suspend fun TestScope.setup(
        target: Host = host, pending: Boolean = false, timing: Timing = Timing(), moshPending: Boolean = false,
        udp: UdpVerdict? = UdpVerdict.OK,
    ): Setup {
        val events = mutableListOf<String>()
        val port = FakePort(events)
        port.caps = port.caps.copy(moshServer = "/usr/bin/mosh-server")
        if (pending) port.capsGate = CompletableDeferred()
        if (moshPending) port.moshServerGate = CompletableDeferred()
        var listener: HostListener? = null
        val holder = HostConnections({ _, l -> listener = l; port }, FakeTrust(), StandardTestDispatcher(testScheduler), UnconfinedTestDispatcher(testScheduler),
            timing = timing)
        holder.connect(target, byteArrayOf(1))
        port.nativeState = HostState.Connected(0u)
        listener!!.onHostStateChanged(HostState.Connected(0u))
        advanceUntilIdle()
        if (udp != null) holder.host(target.id)!!.mutableUdpVerdict.value = udp
        events.clear() // The probe's own calls are not what these tests are about.
        return Setup(holder, port, events, listener!!)
    }

    /** Runs a launch* call to completion and returns what `done` received (recording "navigate" in [Setup.events]). */
    private fun TestScope.activation(s: Setup, start: ((Activation) -> Unit) -> Unit): Activation? {
        var result: Activation? = null
        start { result = it; s.events += "navigate" }
        advanceUntilIdle()
        return result
    }

    private fun TestScope.openAgent(s: Setup, pane: String, session: String? = null) =
        activation(s) { done -> s.activations.launchOpenAgent(7, host.label, session, pane, done) }

    @Test
    fun anAgentTapOnPaneBReusesTheTerminalOpenedOnPaneAOfTheSameSessionAfterFocusingB() = runTest {
        val s = setup()
        val first = (openAgent(s, "w1:p1") as Activation.Ready).terminal
        assertEquals(a, first.target)
        // herdr's focus is shared: a second client on the same session would only show the same pane. B focuses, then
        // the one terminal of the session is shown.
        val second = (openAgent(s, "w1:p2") as Activation.Ready).terminal
        assertSame(first, second)
        assertEquals(listOf("focus:null:w1:p1", "navigate", "focus:null:w1:p2", "navigate"), s.events)

        // Tapping A again: the same terminal, and the focus goes back to A before navigating.
        s.events.clear()
        assertSame(first, (openAgent(s, "w1:p1") as Activation.Ready).terminal)
        assertEquals(listOf("focus:null:w1:p1", "navigate"), s.events)
        assertEquals(1, s.port.terminals.size) // One herdr client for the session, whatever was tapped.
        assertEquals(listOf(null to "w1:p1", null to "w1:p2", null to "w1:p1"), s.port.focused)
        assertNull(s.activations.pending.value)
        s.holder.dismissHost(7)
    }

    @Test
    fun anAgentTapPrefersTheSessionsTerminalOpenedWithoutAPane() = runTest {
        val s = setup()
        s.open(a)
        val whole = s.open(TerminalTarget.Herdr(null, null))
        s.events.clear()
        assertSame(whole, (openAgent(s, "w1:p2") as Activation.Ready).terminal)
        assertEquals(listOf("focus:null:w1:p2", "navigate"), s.events)
        assertEquals(2, s.port.terminals.size) // Duplicates already open are left alone, never closed.
        s.holder.dismissHost(7)
    }

    @Test
    fun anotherSessionOrHostOpensANewTerminal() = runTest {
        val s = setup()
        val default = (openAgent(s, "w1:p1") as Activation.Ready).terminal
        val work = (openAgent(s, "w1:p1", session = "work") as Activation.Ready).terminal
        assertNotSame(default, work)
        assertEquals(TerminalTarget.Herdr("work", "w1:p1"), work.target)
        assertEquals(2, s.port.terminals.size)
        s.holder.dismissHost(7)
    }

    @Test
    fun aClosedOrClosingTerminalIsNeverReusedByAnAgentTap() = runTest {
        val s = setup()
        val closed = (openAgent(s, "w1:p1") as Activation.Ready).terminal
        s.port.terminals.single().second.onStateChanged(SessionState.Closed(CloseReason.Disconnected))
        advanceUntilIdle()
        val fresh = (openAgent(s, "w1:p2") as Activation.Ready).terminal
        assertNotSame(closed, fresh)
        s.holder.disconnectTerminal(fresh) // Being closed: not reused either.
        val third = (openAgent(s, "w1:p1") as Activation.Ready).terminal
        assertNotSame(fresh, third)
        assertEquals(3, s.port.terminals.size)
        s.holder.dismissHost(7)
    }

    @Test
    fun aReattachToAnotherPaneOfTheSessionShowsItsTerminalAsItIs() = runTest {
        val s = setup()
        val open = s.open(a)
        s.events.clear()
        val shown = activation(s) { s.activations.launchReopen(LastTerminal(7, b, TerminalTransport.MOSH), host.label, done = it) }
        assertSame(open, (shown as Activation.Ready).terminal)
        // Not an agent request: herdr's focus is left where the user left it.
        assertEquals(listOf("navigate"), s.events)
        assertEquals(1, s.port.terminals.size)
        s.holder.dismissHost(7)
    }

    @Test
    fun theProgressTextsNameTheTargetByItsTitle() = runTest {
        val s = setup()
        s.port.focusGate = CompletableDeferred()
        s.activations.launchOpenAgent(7, host.label, "work", "w1:p1") {}
        runCurrent()
        assertEquals("Focusing Fixture: herdr work", s.activations.pending.value)
        s.activations.launchReopen(LastTerminal(7, TerminalTarget.Herdr(null, "w1:p2"), TerminalTransport.SSH), host.label) {}
        runCurrent()
        assertEquals("Resuming Fixture: herdr", s.activations.pending.value)
        s.port.focusGate!!.complete(Unit)
        advanceUntilIdle()
        s.holder.dismissHost(7)
    }

    @Test
    fun progressShowsWhileFocusingAndNothingNavigatesUntilHerdrAnswers() = runTest {
        val s = setup()
        s.port.focusGate = CompletableDeferred()
        var result: Activation? = null
        s.activations.launchOpenAgent(7, host.label, null, "w1:p1") { result = it }
        runCurrent()
        assertEquals("Focusing Fixture: herdr", s.activations.pending.value)
        assertNull(result)
        // The terminal starts while the focus is in flight (they share their round trips); nothing is shown yet.
        assertEquals(1, s.port.terminals.size)
        s.port.focusGate!!.complete(Unit)
        advanceUntilIdle()
        assertTrue(result is Activation.Ready)
        assertNull(s.activations.pending.value)
        s.holder.dismissHost(7)
    }

    @Test
    fun aGonePaneShowsAMessageAndOpensOrShowsNothing() = runTest {
        val s = setup()
        val kept = (openAgent(s, "w1:p1") as Activation.Ready).terminal
        s.port.focusFailures["w9:p9"] = HostException.PaneNotFound()
        s.port.focusFailures["w1:p1"] = HostException.PaneNotFound() // A's pane vanishes while its terminal is open.

        // A new agent (in a session with no terminal yet) whose pane is gone: no terminal is opened.
        val failed = openAgent(s, "w9:p9", session = "work") as Activation.Failed
        assertTrue(failed.message, failed.message.contains("pane no longer exists"))
        // The terminal that was started beside the focus is dismissed: no terminal is left for a vanished pane.
        assertEquals(2, s.port.terminals.size)
        assertTrue(s.port.terminals[1].third.destroyed)
        assertEquals(listOf(kept), s.holder.terminals.value)

        // The reused terminal would now show another pane: tapping the row does not navigate to it.
        val reused = openAgent(s, "w1:p1") as Activation.Failed
        assertEquals(failed.message, reused.message)
        assertEquals(2, s.port.terminals.size)
        s.holder.dismissHost(7)
    }

    @Test
    fun otherErrorsShowTheirReasonAndDoNotNavigate() = runTest {
        val s = setup()
        s.port.focusFailures["w1:p1"] = HostException.CommandFailed("herdr session is not running")
        val failed = openAgent(s, "w1:p1") as Activation.Failed
        assertEquals("Could not focus the agent's pane: herdr session is not running", failed.message)
        assertTrue(s.holder.terminals.value.isEmpty())

        s.port.focusFailures["w1:p1"] = HostException.NotInstalled("herdr")
        assertEquals("herdr is not installed on the host.", (openAgent(s, "w1:p1") as Activation.Failed).message)
        s.port.focusFailures["w1:p1"] = HostException.Closed()
        assertEquals("The connection has closed. Reconnect to continue.", (openAgent(s, "w1:p1") as Activation.Failed).message)
        s.port.focusFailures["w1:p1"] = IllegalStateException("boom")
        assertEquals("Could not focus the agent's pane: boom", (openAgent(s, "w1:p1") as Activation.Failed).message)
        assertTrue(s.holder.terminals.value.isEmpty()) // Whatever was started beside the focus is gone.
        s.holder.dismissHost(7)
    }

    @Test
    fun aSpacesTapFocusesTheTabOrThePaneInTheTerminalsSessionAndOpensNothing() = runTest {
        val s = setup()
        val terminal = s.open(TerminalTarget.Herdr("work", null))
        advanceUntilIdle()
        s.events.clear()
        // A tab through herdr's tab focus, an agent through the pane focus, both in the terminal's own session.
        assertNull(s.activations.focusInTerminal(terminal, HerdrFocus.Tab("w2:t1")))
        assertNull(s.activations.focusInTerminal(terminal, HerdrFocus.Pane("w2:p3")))
        assertEquals(listOf("focus-tab:work:w2:t1", "focus:work:w2:p3"), s.events)
        assertEquals(listOf("work" to "w2:t1"), s.port.focusedTabs)
        assertEquals(listOf<Pair<String?, String>>("work" to "w2:p3"), s.port.focused.takeLast(1))
        // Nothing opened, nothing waited for: the terminal is the one on screen.
        assertEquals(listOf(terminal), s.holder.terminals.value)
        assertNull(s.activations.pending.value)

        // What failed is what the user reads.
        s.port.focusFailures["w2:t9"] = HostException.PaneNotFound()
        assertEquals("That tab is no longer open in herdr.", s.activations.focusInTerminal(terminal, HerdrFocus.Tab("w2:t9")))
        s.port.focusFailures["w2:t1"] = HostException.CommandFailed("herdr session is not running")
        assertEquals("Could not focus the tab: herdr session is not running", s.activations.focusInTerminal(terminal, HerdrFocus.Tab("w2:t1")))
        s.port.focusFailures["w2:p9"] = HostException.PaneNotFound()
        assertTrue(s.activations.focusInTerminal(terminal, HerdrFocus.Pane("w2:p9"))!!.contains("pane no longer exists"))

        // A shell terminal has no Spaces: nothing is sent.
        val shell = s.open(TerminalTarget.Shell)
        advanceUntilIdle()
        s.events.clear()
        assertNull(s.activations.focusInTerminal(shell, HerdrFocus.Tab("w2:t1")))
        assertTrue(s.events.none { it.startsWith("focus") })

        // A host that went away is said so, before anything is sent.
        s.holder.dismissHost(7)
        s.events.clear()
        assertEquals("Fixture is no longer connected.", s.activations.focusInTerminal(terminal, HerdrFocus.Tab("w2:t1")))
        assertTrue(s.events.none { it.startsWith("focus") })
    }

    @Test
    fun aHostThatIsNoLongerConnectedIsReportedBeforeAnyFocus() = runTest {
        val s = setup()
        s.holder.dismissHost(7)
        s.events.clear()
        assertEquals(Activation.Failed("Fixture is no longer connected."), openAgent(s, "w1:p1"))
        assertTrue(s.events.none { it.startsWith("focus") })
    }

    @Test
    fun aTerminalOpenedFromTheHostScreenOpensAtOnceWithoutWaitingForAnyProbe() = runTest {
        val s = setup(pending = true, moshPending = true, udp = null)
        var result: Activation? = null
        s.activations.launchOpen(s.holder.host(7)!!, TerminalTarget.Tmux("work")) { result = it }
        runCurrent()
        // Neither the capability probe nor `mosh_server()` has answered: tmux opens over SSH now, mosh behind it.
        val terminal = (result as Activation.Ready).terminal
        assertEquals(TerminalTransport.SSH, terminal.transport.value)
        assertEquals(listOf(TerminalTransport.SSH, TerminalTransport.MOSH), s.port.transports)
        assertNull(s.activations.pending.value)

        // A failure to open is a message, never a navigation.
        s.port.openFailure = HostException.Closed()
        assertEquals(Activation.Failed("The connection has closed. Reconnect to continue."), s.activations.open(s.holder.host(7)!!, TerminalTarget.Shell))
        s.holder.dismissHost(7)
    }

    @Test
    fun anInboxTapNeverWaitsAndOnlyAResumeThatConnectedTheHostAwaitsMoshServer() = runTest {
        val s = setup(pending = true, moshPending = true, udp = null)
        var result: Activation? = null
        s.activations.launchOpenAgent(7, host.label, null, "w1:p1") { result = it }
        runCurrent()
        // An inbox tap on a live host: opened and shown with nothing answered yet.
        assertEquals(TerminalTransport.SSH, (result as Activation.Ready).terminal.transport.value)

        // A reattach on a host that was already connected does not wait either: the shell takes SSH.
        var shown: Activation? = null
        s.activations.launchReopen(LastTerminal(7, TerminalTarget.Shell, TerminalTransport.MOSH), host.label) { shown = it }
        runCurrent()
        assertEquals(TerminalTransport.SSH, (shown as Activation.Ready).terminal.transport.value)

        // A Resume that connected the host waits for `mosh_server()` alone (one round trip), so its shell
        // can choose mosh; the capability probe is still unanswered.
        val r = setup(pending = true, moshPending = true, udp = null)
        var reopened: Activation? = null
        r.activations.launchReopen(LastTerminal(7, TerminalTarget.Shell, TerminalTransport.SSH), host.label, connectedInThisTap = true) { reopened = it }
        runCurrent()
        assertNull(reopened)
        assertTrue(r.port.terminals.isEmpty())
        r.port.moshServerGate!!.complete(Unit)
        advanceUntilIdle()
        // What the target ran over before does not decide: SSH was remembered, AUTO with mosh-server picks mosh.
        assertEquals(TerminalTransport.MOSH, (reopened as Activation.Ready).terminal.transport.value)
        assertNull(r.holder.host(7)!!.capabilities.value)
        s.holder.dismissHost(7)
        r.holder.dismissHost(7)
    }

    @Test
    fun cancelAndANewerRequestDropTheWaitWithoutNavigating() = runTest {
        val s = setup()
        s.port.focusGate = CompletableDeferred()
        val done = mutableListOf<String>()
        s.activations.launchOpenAgent(7, host.label, null, "w1:p1") { done += "first" }
        runCurrent()
        s.activations.cancel() // The user navigated elsewhere.
        assertNull(s.activations.pending.value)
        s.port.focusGate!!.complete(Unit)
        advanceUntilIdle()
        assertTrue(done.isEmpty())
        assertTrue(s.holder.terminals.value.isEmpty()) // A terminal started beside the focus is dismissed with the wait.

        s.port.focusGate = CompletableDeferred()
        s.activations.launchOpenAgent(7, host.label, null, "w1:p1") { done += "older" }
        runCurrent()
        s.activations.launchOpenAgent(7, host.label, null, "w1:p2") { done += "newer" }
        runCurrent()
        assertEquals("Focusing Fixture: herdr", s.activations.pending.value)
        s.port.focusGate!!.complete(Unit)
        advanceUntilIdle()
        assertEquals(listOf("newer"), done)
        assertEquals(listOf<TerminalTarget>(b), s.holder.terminals.value.map { it.target })
        assertNull(s.activations.pending.value)
        s.holder.dismissHost(7)
    }

    @Test
    fun aNewAgentTerminalIsOpenedWhileItsPaneIsBeingFocusedAndOnlyShownOnceBothAreDone() = runTest {
        val s = setup()
        s.port.focusGate = CompletableDeferred()
        val shown = mutableListOf<Activation>()
        s.activations.launchOpenAgent(7, host.label, null, "w1:p1") { shown += it }
        runCurrent()
        // The focus is in flight and so is the terminal (mosh over AUTO): the wait is the longer of the two.
        assertEquals(listOf("focus:null:w1:p1"), s.events)
        assertEquals(1, s.port.terminals.size)
        assertEquals(TerminalTransport.MOSH, s.port.transports.single())
        assertTrue(shown.isEmpty())
        s.port.focusGate!!.complete(Unit)
        advanceUntilIdle()
        assertEquals(s.holder.terminals.value.single(), (shown.single() as Activation.Ready).terminal)
        s.holder.dismissHost(7)
    }

    @Test
    fun anOpenTerminalIsOnlyRefocusedNeverOpenedAgainOrAlongsideANewOne() = runTest {
        val s = setup()
        val first = (openAgent(s, "w1:p1") as Activation.Ready).terminal
        s.events.clear()
        s.port.focusGate = CompletableDeferred()
        var shown: Activation? = null
        s.activations.launchOpenAgent(7, host.label, null, "w1:p1") { shown = it }
        runCurrent()
        assertEquals(listOf("focus:null:w1:p1"), s.events)
        assertEquals(1, s.port.terminals.size) // Nothing new was opened.
        s.port.focusGate!!.complete(Unit)
        advanceUntilIdle()
        assertSame(first, (shown as Activation.Ready).terminal)
        s.holder.dismissHost(7)
    }

    @Test
    fun theTapPathMarksFocusTerminalAndFrameInOrderWithItsMilliseconds() = runTest {
        val lines = mutableListOf<String>()
        var clock = 1_000L
        val s = setup(timing = Timing(lines::add) { clock })
        lines.clear() // The connect's own markers are not what this is about.
        s.port.focusGate = CompletableDeferred()
        var shown: Activation? = null
        s.activations.launchOpenAgent(7, host.label, null, "w1:p1") { shown = it }
        runCurrent()
        clock += 300 // herdr acknowledges the focus
        s.port.focusGate!!.complete(Unit)
        runCurrent()
        val terminal = s.holder.terminals.value.single()
        clock += 200 // the mosh terminal connects
        s.port.terminals[0].second.onStateChanged(SessionState.Connected)
        advanceUntilIdle()
        clock += 150 // the first frame is drawn
        s.holder.timing.terminalFrame(terminal.id)
        assertTrue(shown is Activation.Ready)
        assertEquals(
            listOf(
                "tap host=7 pane=w1:p1 begin ms=0",
                "tap host=7 pane=w1:p1 focused ms=300",
                "tap host=7 pane=w1:p1 terminal-connected ms=500",
                "tap host=7 pane=w1:p1 frame ms=650",
            ),
            lines,
        )
        // The path is over: a later frame (or a second view) adds nothing.
        s.holder.timing.terminalFrame(terminal.id)
        assertEquals(4, lines.size)
        s.holder.dismissHost(7)
    }

    /** What the session picker's choice of [target] activates on host 7 (the host screen's and Home's picker alike). */
    private suspend fun pick(s: Setup, target: TerminalTarget): ActiveTerminal =
        (s.activations.open(s.holder.host(7)!!, target) as Activation.Ready).terminal

    @Test
    fun thePickerReusesAnOpenTmuxOrHerdrSessionAndAlwaysOpensANewShell() = runTest {
        val s = setup()
        val tmux = pick(s, TerminalTarget.Tmux("work"))
        assertSame(tmux, pick(s, TerminalTarget.Tmux("work"))) // The same session: brought to the front.
        assertNotSame(tmux, pick(s, TerminalTarget.Tmux("other")))
        val herdr = pick(s, TerminalTarget.Herdr("personal", null))
        assertSame(herdr, pick(s, TerminalTarget.Herdr("personal", null)))
        assertNotSame(herdr, pick(s, TerminalTarget.Herdr(null, null))) // The default session is another session.
        // A herdr terminal opened on a pane (an inbox tap) is that session's terminal too, shown as it is: the picker
        // is no agent request, so herdr's focus stays where the user left it.
        val pane = s.open(TerminalTarget.Herdr("work", "w1:p3"))
        s.events.clear()
        assertSame(pane, pick(s, TerminalTarget.Herdr("work", null)))
        assertTrue(s.events.none { it.startsWith("focus") })
        // A shell is never reused.
        val shell = pick(s, TerminalTarget.Shell)
        assertNotSame(shell, pick(s, TerminalTarget.Shell))
        val directoryShell = pick(s, TerminalTarget.ShellIn("/work/project"))
        assertNotSame(directoryShell, pick(s, TerminalTarget.ShellIn("/work/project")))
        assertEquals(9, s.holder.terminals.value.size) // tmux x2, herdr x2, the pane, shell x2, directory shell x2.
        s.holder.dismissHost(7)
    }

    @Test
    fun thePickersAgentTakesTheInboxPathAndItsWholeSessionRowShowsThatTerminalAsItIs() = runTest {
        val s = setup()
        val calls = mutableListOf<String>()
        val shown = mutableListOf<Activation>()
        val choices = PickerChoices(
            host,
            open = { target -> calls += "open:$target"; s.activations.launchOpen(s.holder.host(7)!!, target) { shown += it } },
            // Or2App's openAgent: the Inbox row's path.
            openAgent = { hostId, label, session, pane ->
                calls += "agent:$hostId:$label:$session:$pane"
                s.activations.launchOpenAgent(hostId, label, session, pane) { shown += it }
            },
            dismiss = { calls += "dismiss" },
        )
        choices.agent("work", "w1:p2")
        advanceUntilIdle()
        // The sheet closes, then the agent's pane is focused and its session's terminal opens.
        assertEquals(listOf("dismiss", "agent:7:${host.label}:work:w1:p2"), calls)
        val agent = (shown.single() as Activation.Ready).terminal
        assertEquals(TerminalTarget.Herdr("work", "w1:p2"), agent.target)
        assertEquals(listOf("work" to "w1:p2"), s.port.focused)

        // Whole session: the same terminal, shown as it is (no focus, no second client).
        calls.clear()
        choices.herdr("work")
        advanceUntilIdle()
        assertEquals(listOf("dismiss", "open:${TerminalTarget.Herdr("work", null)}"), calls)
        assertSame(agent, (shown.last() as Activation.Ready).terminal)
        assertEquals(listOf("work" to "w1:p2"), s.port.focused)

        // Another agent of the session: focused, and the same terminal again.
        choices.agent("work", "w2:p1")
        advanceUntilIdle()
        assertSame(agent, (shown.last() as Activation.Ready).terminal)
        assertEquals(listOf("work" to "w1:p2", "work" to "w2:p1"), s.port.focused)
        assertEquals(1, s.port.terminals.size)
        s.holder.dismissHost(7)
    }

    @Test
    fun aClosedOrClosingTerminalIsNotReusedByThePicker() = runTest {
        val s = setup()
        val first = pick(s, TerminalTarget.Tmux("work"))
        s.port.terminals.single().second.onStateChanged(SessionState.Closed(CloseReason.Disconnected))
        advanceUntilIdle()
        val second = pick(s, TerminalTarget.Tmux("work"))
        assertNotSame(first, second) // Closed: a fresh terminal.
        val herdr = pick(s, TerminalTarget.Herdr(null, null))
        s.holder.disconnectTerminal(herdr) // Being closed: not reused either.
        assertNotSame(herdr, pick(s, TerminalTarget.Herdr(null, null)))
        assertSame(second, pick(s, TerminalTarget.Tmux("work")))
        s.holder.dismissHost(7)
    }

    @Test
    fun closingAnOpenTerminalDisconnectsAndDismissesItAndAClosedOneIsOnlyDismissed() = runTest {
        val s = setup()
        val tmux = pick(s, TerminalTarget.Tmux("work"))
        val tmuxSession = s.port.terminals.last().third
        s.activations.close(tmux)
        assertTrue("disconnect" in tmuxSession.events) // Rust stops its mosh server; the tmux session itself runs on.
        assertFalse(tmux in s.holder.terminals.value)

        // A shell that has closed by itself (its final frame still shown): the × only takes it away.
        val shell = pick(s, TerminalTarget.Shell)
        s.port.terminals.last().second.onStateChanged(SessionState.Closed(CloseReason.Disconnected))
        advanceUntilIdle()
        assertTrue(shell in s.holder.terminals.value)
        s.activations.close(shell)
        assertTrue(s.holder.terminals.value.isEmpty())
        assertTrue(s.port.terminals.last().third.destroyed)
        s.holder.dismissHost(7)
    }

    @Test
    fun onlyAnOpenShellAsksBeforeItCloses() {
        assertTrue(closeAsks(TerminalTarget.ShellIn("/work/project"), closed = false))
        assertFalse(closeAsks(TerminalTarget.ShellIn("/work/project"), closed = true))
        assertTrue(closeAsks(TerminalTarget.Shell, closed = false))
        assertFalse(closeAsks(TerminalTarget.Shell, closed = true))
        assertFalse(closeAsks(TerminalTarget.Tmux("main"), closed = false))
        assertFalse(closeAsks(TerminalTarget.Herdr(null, null), closed = false))
        assertFalse(closeAsks(TerminalTarget.Herdr("work", "w1:p1"), closed = false))
    }
}
