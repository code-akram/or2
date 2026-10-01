package io.github.code_akram.or2.app

import io.github.code_akram.or2.connection.FakePort
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
 * Every path into an agent terminal awaits `focusHerdrPane` for its pane first: tapping the inbox
 * row (new or reused terminal), the session switcher, a Home thumbnail and the host screen's
 * recent list all go through [TerminalActivations]; a gone pane or a failed focus shows a message
 * and never an activation.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class TerminalActivationsTest {
    private val host = testHost()
    private val a = TerminalTarget.Herdr(null, "w1:p1")
    private val b = TerminalTarget.Herdr(null, "w1:p2")

    private class Setup(val holder: HostConnections, val port: FakePort, val events: MutableList<String>) {
        val activations get() = holder.activations
        fun open(target: TerminalTarget) = holder.openTerminal(holder.host(7)!!, target)
    }

    private suspend fun TestScope.setup(): Setup {
        val events = mutableListOf<String>()
        val port = FakePort(events)
        val holder = HostConnections({ _, _ -> port }, FakeTrust(), StandardTestDispatcher(testScheduler), UnconfinedTestDispatcher(testScheduler))
        holder.connect(host, byteArrayOf(1))
        port.nativeState = HostState.Connected(0u)
        return Setup(holder, port, events)
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
    fun agentAThenBThenAReusesATerminalAndFocusesAFirst() = runTest {
        val s = setup()
        val first = (openAgent(s, "w1:p1") as Activation.Ready).terminal
        val second = (openAgent(s, "w1:p2") as Activation.Ready).terminal
        assertEquals(a, first.target)
        assertEquals(b, second.target)
        assertNotSame(first, second)
        assertEquals(listOf("focus:null:w1:p1", "navigate", "focus:null:w1:p2", "navigate"), s.events)

        // Tapping A again: A's terminal is reused, and the focus goes back to A before navigating.
        s.events.clear()
        val again = (openAgent(s, "w1:p1") as Activation.Ready).terminal
        assertSame(first, again)
        assertEquals(listOf("focus:null:w1:p1", "navigate"), s.events)
        assertEquals(2, s.port.terminals.size) // No third terminal.
        assertEquals(listOf(null to "w1:p1", null to "w1:p2", null to "w1:p1"), s.port.focused)
        assertNull(s.activations.pending.value)
        s.holder.dismissHost(7)
    }

    @Test
    fun progressShowsWhileFocusingAndNothingNavigatesUntilHerdrAnswers() = runTest {
        val s = setup()
        s.port.focusGate = CompletableDeferred()
        var result: Activation? = null
        s.activations.launchOpenAgent(7, host.label, null, "w1:p1") { result = it }
        runCurrent()
        assertEquals("Focusing Fixture: herdr w1:p1", s.activations.pending.value)
        assertNull(result)
        assertTrue(s.port.terminals.isEmpty()) // Not even opened before the focus succeeded.
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

        // A new agent whose pane is gone: no terminal is opened.
        val failed = openAgent(s, "w9:p9") as Activation.Failed
        assertTrue(failed.message, failed.message.contains("pane no longer exists"))
        assertEquals(1, s.port.terminals.size)
        assertEquals(listOf(kept), s.holder.terminals.value)

        // The reused terminal would now show another pane: tapping the row does not navigate to it.
        val reused = openAgent(s, "w1:p1") as Activation.Failed
        assertEquals(failed.message, reused.message)
        assertEquals(1, s.port.terminals.size)
        s.holder.dismissHost(7)
    }

    @Test
    fun otherErrorsShowTheirReasonAndDoNotNavigate() = runTest {
        val s = setup()
        s.port.focusFailures["w1:p1"] = HostException.CommandFailed("herdr session is not running")
        val failed = openAgent(s, "w1:p1") as Activation.Failed
        assertEquals("Could not focus the agent's pane: herdr session is not running", failed.message)
        assertTrue(s.port.terminals.isEmpty())

        s.port.focusFailures["w1:p1"] = HostException.NotInstalled("herdr")
        assertEquals("herdr is not installed on the host.", (openAgent(s, "w1:p1") as Activation.Failed).message)
        s.port.focusFailures["w1:p1"] = HostException.Closed()
        assertEquals("The connection has closed. Reconnect to continue.", (openAgent(s, "w1:p1") as Activation.Failed).message)
        s.port.focusFailures["w1:p1"] = IllegalStateException("boom")
        assertEquals("Could not focus the agent's pane: boom", (openAgent(s, "w1:p1") as Activation.Failed).message)
        assertTrue(s.port.terminals.isEmpty())
        s.holder.dismissHost(7)
    }

    @Test
    fun aHostThatIsNoLongerConnectedIsReportedBeforeAnyFocus() = runTest {
        val s = setup()
        val open = s.open(a)
        s.holder.dismissHost(7)
        s.events.clear()
        assertEquals(Activation.Failed("Fixture is no longer connected."), openAgent(s, "w1:p1"))
        assertEquals(Activation.Failed("Fixture is no longer connected."), activation(s) { s.activations.launchReuse(open, it) })
        assertTrue(s.events.none { it.startsWith("focus") })
    }

    @Test
    fun theSwitcherAndAThumbnailResumeFocusTheTerminalsPaneBeforeNavigating() = runTest {
        val s = setup()
        val first = s.open(a)
        val second = s.open(b)
        val shell = s.open(TerminalTarget.Shell)
        s.events.clear()

        // Switcher: B is on screen, A is chosen. Then the Home thumbnail of B.
        assertEquals(Activation.Ready(first), activation(s) { s.activations.launchReuse(first, it) })
        assertEquals(Activation.Ready(second), activation(s) { s.activations.launchReuse(second, it) })
        assertEquals(listOf("focus:null:w1:p1", "navigate", "focus:null:w1:p2", "navigate"), s.events)

        // Named sessions keep their name.
        val named = s.open(TerminalTarget.Herdr("work", "w2:p1"))
        s.events.clear()
        assertEquals(Activation.Ready(named), activation(s) { s.activations.launchReuse(named, it) })
        assertEquals(listOf("focus:work:w2:p1", "navigate"), s.events)

        // Terminals that are not for one herdr pane have nothing to focus: shell, tmux, a herdr session picked as a whole.
        val tmux = s.open(TerminalTarget.Tmux("work"))
        val whole = s.open(TerminalTarget.Herdr("work", null))
        s.events.clear()
        for (terminal in listOf(shell, tmux, whole)) {
            assertEquals(Activation.Ready(terminal), activation(s) { s.activations.launchReuse(terminal, it) })
        }
        assertEquals(listOf("navigate", "navigate", "navigate"), s.events)
        s.holder.dismissHost(7)
    }

    @Test
    fun aFailedFocusKeepsYouWhereYouAreAndAClosedTerminalNeedsNoFocus() = runTest {
        val s = setup()
        val terminal = s.open(a)
        s.port.focusFailures["w1:p1"] = HostException.PaneNotFound()
        assertTrue(activation(s) { s.activations.launchReuse(terminal, it) } is Activation.Failed)

        // A closed terminal shows its final frame: nothing to focus, nothing to fail.
        terminal.mutableState.value = SessionState.Closed(CloseReason.Disconnected)
        s.events.clear()
        assertEquals(Activation.Ready(terminal), activation(s) { s.activations.launchReuse(terminal, it) })
        assertEquals(listOf("navigate"), s.events)
        s.holder.dismissHost(7)
    }

    @Test
    fun aTerminalDismissedWhileFocusingIsNotShown() = runTest {
        val s = setup()
        val terminal = s.open(a)
        s.port.focusGate = CompletableDeferred()
        var result: Activation? = null
        s.activations.launchReuse(terminal) { result = it }
        runCurrent()
        s.holder.dismissTerminal(terminal)
        s.port.focusGate!!.complete(Unit)
        advanceUntilIdle()
        assertEquals(Activation.Failed("That terminal is no longer open."), result)
        s.holder.dismissHost(7)
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
        assertTrue(s.port.terminals.isEmpty())

        s.port.focusGate = CompletableDeferred()
        s.activations.launchOpenAgent(7, host.label, null, "w1:p1") { done += "older" }
        runCurrent()
        s.activations.launchOpenAgent(7, host.label, null, "w1:p2") { done += "newer" }
        runCurrent()
        assertEquals("Focusing Fixture: herdr w1:p2", s.activations.pending.value)
        s.port.focusGate!!.complete(Unit)
        advanceUntilIdle()
        assertEquals(listOf("newer"), done)
        assertEquals(listOf<TerminalTarget>(b), s.port.terminals.map { it.first })
        assertNull(s.activations.pending.value)
        s.holder.dismissHost(7)
    }

    @Test
    fun suspendingFormsMatchTheLaunchedOnes() = runTest {
        val s = setup()
        val first = (s.activations.openAgent(7, host.label, null, "w1:p1") as Activation.Ready).terminal
        assertEquals(Activation.Ready(first), s.activations.reuse(first))
        assertEquals(listOf("focus:null:w1:p1", "focus:null:w1:p1"), s.events)
        s.holder.dismissHost(7)
    }
}
