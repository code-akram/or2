package io.github.code_akram.or2.app

import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.SessionFailure
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.ffi.TerminalTransport
import org.junit.Assert.*
import org.junit.Test

/** Reattach: what is remembered, what is decided on return, and the one-time prompts. */
class ReattachTest {
    private val pane = TerminalTarget.Herdr("work", "w1:p2")
    private val last = LastTerminal(7, pane, TerminalTransport.MOSH)

    @Test
    fun theLastTerminalRoundTripsThroughItsEncodingForEveryTarget() {
        val targets = listOf(
            TerminalTarget.Shell, TerminalTarget.Tmux("work-1"), TerminalTarget.Herdr(null, null),
            TerminalTarget.Herdr("work", "w1:p2"), TerminalTarget.Herdr(null, "w1:p2"), TerminalTarget.Herdr("a|b", "p:1=2"),
            TerminalTarget.Tmux("é ü"),
        )
        for (target in targets) for (transport in TerminalTransport.entries) {
            val original = LastTerminal(42, target, transport)
            assertEquals(original, LastTerminal.decode(original.encode()))
        }
    }

    @Test
    fun anythingThatIsNotOurEncodingDecodesToNothing() {
        for (text in listOf(null, "", "garbage", "x|SSH|shell", "7|TELNET|shell", "7|SSH|tmux", "7|SSH|tmux|-", "7|SSH|herdr|-", "7|SSH|shell|extra", "7|SSH|zsh")) {
            assertNull(text, LastTerminal.decode(text))
        }
    }

    @Test
    fun memoryPersistsAcrossInstancesAndForgetsWhatTheUserEnds() {
        val store = MemoryPrefStore()
        val memory = ReattachMemory(store)
        assertNull(memory.last.value)
        memory.remember(last)
        assertEquals(last, ReattachMemory(store).last.value) // Survives the process.

        memory.terminalClosed(7, TerminalTarget.Shell) // A different terminal on the same host.
        memory.terminalClosed(8, pane) // The same target on another host.
        assertEquals(last, memory.last.value)
        memory.terminalClosed(7, pane)
        assertNull(memory.last.value)
        assertNull(ReattachMemory(store).last.value)

        memory.remember(last)
        memory.hostClosed(8)
        assertEquals(last, memory.last.value)
        memory.hostClosed(7)
        assertNull(ReattachMemory(store).last.value)
    }

    private fun session(id: Long, target: TerminalTarget = pane, host: Long = 7, alive: Boolean = true) = OpenSession(id, host, target, alive)

    @Test
    fun anAliveSessionIsShownWhateverTheHostDoes() {
        assertEquals(Reattach.Show(3), decideReattach(last, listOf(session(2, TerminalTarget.Shell), session(3)), emptySet(), setOf(7)))
        assertEquals(Reattach.Show(3), decideReattach(last, listOf(session(3)), setOf(7), setOf(7)))
    }

    @Test
    fun aConnectedHostReopensTheSameTarget() {
        assertEquals(Reattach.Reopen(last), decideReattach(last, emptyList(), setOf(7), setOf(7)))
        // A closed session of it, or an alive one for another target or host, does not count.
        assertEquals(Reattach.Reopen(last), decideReattach(last, listOf(session(3, alive = false), session(4, TerminalTarget.Shell), session(5, host = 8)), setOf(7), setOf(7, 8)))
    }

    @Test
    fun anUnconnectedHostOffersResume() {
        assertEquals(Reattach.Resume(last), decideReattach(last, emptyList(), emptySet(), setOf(7)))
        assertEquals(Reattach.Resume(last), decideReattach(last, listOf(session(3, alive = false)), setOf(8), setOf(7, 8)))
    }

    @Test
    fun nothingRememberedOrADeletedHostIsLeftAlone() {
        assertEquals(Reattach.None, decideReattach(null, listOf(session(3)), setOf(7), setOf(7)))
        assertEquals(Reattach.None, decideReattach(last, listOf(session(3)), setOf(7), setOf(8)))
    }

    private val connected = HostState.Connected(0u)
    private val lost = HostState.Closed(CloseReason.Failed(SessionFailure.ConnectionLost("x")))

    @Test
    fun resumeWaitsForTheUnlockThenOpensOrGivesUp() {
        assertEquals(ResumeStep.WAIT, resumeStep(null, busy = true)) // Biometric prompt up.
        assertEquals(ResumeStep.WAIT, resumeStep(lost, busy = true)) // The old lost connection, until the new attempt replaces it.
        assertEquals(ResumeStep.WAIT, resumeStep(HostState.Connecting, busy = false)) // connect() returned, the host is on its way.
        assertEquals(ResumeStep.WAIT, resumeStep(HostState.Authenticating, busy = false))
        assertEquals(ResumeStep.OPEN, resumeStep(connected, busy = false))
        assertEquals(ResumeStep.OPEN, resumeStep(connected, busy = true))
        assertEquals(ResumeStep.ABORT, resumeStep(null, busy = false)) // Biometric cancelled.
        assertEquals(ResumeStep.ABORT, resumeStep(lost, busy = false)) // Connect failed.
    }

    @Test
    fun notificationPermissionIsAskedOnceOnAndroid13Up() {
        val store = MemoryPrefStore()
        val policy = NotificationPermissionPolicy(store)
        assertFalse(policy.shouldAsk(sdk = 32, granted = false))
        assertFalse(policy.shouldAsk(sdk = 34, granted = true))
        assertTrue(policy.shouldAsk(sdk = 34, granted = false))
        policy.markAsked()
        assertFalse(policy.shouldAsk(sdk = 34, granted = false)) // Denied once: never asked again.
        assertFalse(NotificationPermissionPolicy(store).shouldAsk(sdk = 36, granted = false)) // Persisted.
    }

    @Test
    fun theBatteryExplanationIsDueOnlyAfterABackgroundedSessionAndShownOnce() {
        val store = MemoryPrefStore()
        var exempt = false
        val prompt = BatteryPrompt(store) { exempt }
        assertFalse(prompt.takeIfDue()) // Nothing happened yet.
        prompt.onBackgrounded(sessionOpen = false)
        assertFalse(prompt.takeIfDue()) // No session open: nothing to protect.

        prompt.onBackgrounded(sessionOpen = true)
        assertTrue(BatteryPrompt(store) { exempt }.takeIfDue().also { /* a new instance sees the persisted flag */ })
        assertFalse(prompt.takeIfDue()) // Shown: never again.
        prompt.onBackgrounded(sessionOpen = true)
        assertFalse(prompt.takeIfDue())
    }

    @Test
    fun anAlreadyExemptAppIsNeverAsked() {
        val store = MemoryPrefStore()
        val prompt = BatteryPrompt(store) { true }
        prompt.onBackgrounded(sessionOpen = true)
        assertFalse(prompt.takeIfDue())
        var exempt = true
        val later = BatteryPrompt(store) { exempt }
        exempt = false // Exemption withdrawn later: still counts as asked, no nagging.
        later.onBackgrounded(sessionOpen = true)
        assertFalse(later.takeIfDue())
    }

    @Test
    fun exemptionGrantedWhileTheExplanationWasDueStopsTheDialog() {
        val store = MemoryPrefStore()
        var exempt = false
        val prompt = BatteryPrompt(store) { exempt }
        prompt.onBackgrounded(sessionOpen = true)
        exempt = true // The user granted it in system settings meanwhile.
        assertFalse(prompt.takeIfDue())
        exempt = false
        assertFalse(prompt.takeIfDue()) // And it was consumed.
    }
}
