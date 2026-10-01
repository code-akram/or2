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
    fun aPendingResumeTargetSurvivesTheSavedStateRoundTripAndNothingStaysNothing() {
        // Rotation (or process death) during a cold resume: the continuation is saved state, not
        // `remember`, so the recreated composition still knows which terminal to reopen.
        val scope = androidx.compose.runtime.saveable.SaverScope { true }
        for (target in listOf(last, LastTerminal(3, TerminalTarget.Shell, TerminalTransport.SSH), LastTerminal(4, TerminalTarget.Tmux("a b"), TerminalTransport.MOSH))) {
            val saved = with(PendingResumeSaver) { scope.save(target) }
            assertNotNull(saved)
            assertEquals(target, PendingResumeSaver.restore(saved!!))
        }
        assertNull(with(PendingResumeSaver) { scope.save(null) }) // Nothing pending: nothing saved.
        assertNull(PendingResumeSaver.restore("garbage")) // Not ours: nothing pending.
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

    // --- the battery exemption, asked up front ----------------------------------------------------

    @Test
    fun theBatteryExplanationIsAskedBeforeTheFirstConnectionAndOnlyOnce() {
        val store = MemoryPrefStore()
        val prompt = BatteryPrompt(store) { false }
        assertTrue(prompt.shouldExplain()) // The first time a connection starts.
        assertFalse(prompt.explaining.value)
        prompt.explain()
        assertTrue(prompt.explaining.value) // Up on screen, waiting for the answer.
        prompt.explained()
        assertFalse(prompt.explaining.value)
        assertFalse(prompt.shouldExplain()) // Never again...
        assertFalse(BatteryPrompt(store) { false }.shouldExplain()) // ...also after a restart.
    }

    @Test
    fun aRecreatedActivityWaitingOnTheExplanationShowsItAgainInsteadOfStayingBusy() {
        // The process died while the explanation was up: nothing was answered, nothing recorded,
        // and the new process's prompt starts with no explanation on screen.
        val store = MemoryPrefStore()
        val restored = BatteryPrompt(store) { false }
        assertFalse(restored.explaining.value)
        assertEquals(BatteryStage.EXPLANATION, restored.restoreStage())
        assertTrue(restored.explaining.value) // On screen again, awaiting the answer.
        restored.explained()
        assertFalse(restored.explaining.value)
        // Same process (rotation): the explanation is still up and restoring it changes nothing.
        val live = BatteryPrompt(MemoryPrefStore()) { false }
        live.explain()
        assertEquals(BatteryStage.EXPLANATION, live.restoreStage())
        assertTrue(live.explaining.value)
    }

    @Test
    fun aRestoredConnectAfterTheExplanationWaitsForTheSystemRequestAndNeverRelaunchesIt() {
        val store = MemoryPrefStore()
        val prompt = BatteryPrompt(store) { false }
        prompt.explain()
        prompt.explained() // "Allow" was tapped: the system dialog is (or was) up.
        val restored = BatteryPrompt(store) { false }
        assertEquals(BatteryStage.SYSTEM_REQUEST, restored.restoreStage())
        assertFalse(restored.explaining.value) // Nothing of ours to show; the result callback carries on.
    }

    @Test
    fun aRestoredConnectWhoseAppBecameExemptJustConnects() {
        val restored = BatteryPrompt(MemoryPrefStore()) { true }
        assertEquals(BatteryStage.PROCEED, restored.restoreStage())
        assertFalse(restored.explaining.value)
    }

    @Test
    fun anAlreadyExemptAppIsNeverAskedAndNeverShowsTheCard() {
        val prompt = BatteryPrompt(MemoryPrefStore()) { true }
        assertFalse(prompt.shouldExplain())
        prompt.declined()
        assertFalse(prompt.card.value)
    }

    @Test
    fun aDeclinedExemptionLeavesADismissibleCardUntilItIsGranted() {
        val store = MemoryPrefStore()
        var exempt = false
        val prompt = BatteryPrompt(store) { exempt }
        assertFalse(prompt.card.value) // Nothing declined yet.
        prompt.explained()
        assertFalse(prompt.card.value) // Allow was tapped: the system dialog decides.
        prompt.declined() // "Not now", or the system dialog was refused.
        assertTrue(prompt.card.value)
        assertTrue(BatteryPrompt(store) { exempt }.card.value) // Persisted.
        assertFalse(prompt.shouldExplain()) // The explanation is still not repeated.

        exempt = true // Granted from the card, in the system dialog or in Settings.
        prompt.refresh()
        assertFalse(prompt.card.value)
        exempt = false // Withdrawn later: the card comes back (declined, not dismissed).
        prompt.refresh()
        assertTrue(prompt.card.value)

        prompt.dismissCard()
        assertFalse(prompt.card.value)
        assertFalse(BatteryPrompt(store) { exempt }.card.value) // Dismissed for good.
    }

    // --- cold launch: the marker and the decision -------------------------------------------------

    @Test
    fun theSessionMarkerIsSetWhileSessionsAreOpenAndClearedOnAnOrderlyEnd() {
        val store = MemoryPrefStore()
        val first = SessionMarker(store)
        assertFalse(first.diedWithSessions) // A first run, or an orderly end before.
        first.onOpenSessions(true)
        assertTrue(store.getBoolean("sessions_open"))
        first.onOpenSessions(false) // Disconnect all, the last close, a remote exit.
        assertFalse(store.getBoolean("sessions_open"))
        assertFalse(SessionMarker(store).diedWithSessions)
    }

    @Test
    fun aKilledProcessLeavesTheMarkerAndTheNextColdStartTakesItOnce() {
        val store = MemoryPrefStore()
        SessionMarker(store).onOpenSessions(true) // ... and the process is killed here.
        val next = SessionMarker(store)
        assertTrue(next.diedWithSessions)
        // The new process starts idle and clears the marker; what it found is unaffected.
        next.onOpenSessions(false)
        assertFalse(store.getBoolean("sessions_open"))
        assertTrue(next.diedWithSessions)
        assertTrue(next.takeColdResume()) // The first activity resumes...
        assertFalse(next.takeColdResume()) // ...a recreated one (rotation) does not.
        assertFalse(SessionMarker(store).diedWithSessions) // And the next process finds nothing to resume.
    }

    @Test
    fun anUnchangedOpenStateIsNotWrittenAgain() {
        val writes = mutableListOf<Boolean>()
        val store = object : PrefStore by MemoryPrefStore() {
            override fun putBoolean(key: String, value: Boolean) { writes += value }
        }
        val marker = SessionMarker(store)
        marker.onOpenSessions(false)
        marker.onOpenSessions(true)
        marker.onOpenSessions(true)
        marker.onOpenSessions(false)
        assertEquals(listOf(true, false), writes)
    }

    @Test
    fun aColdLauncherStartResumesOnlyAfterADeathWithSessionsOpenAndARememberedTargetItCanUnlock() {
        val hosts = listOf(host(7))
        assertTrue(shouldAutoResumeOnLaunch(true, last, hosts, emptySet()))
        // An orderly end, or a first run: the Resume card at most.
        assertFalse(shouldAutoResumeOnLaunch(false, last, hosts, emptySet()))
        // The user closed what was open (nothing is remembered), or deleted the host, or it has no key.
        assertFalse(shouldAutoResumeOnLaunch(true, null, hosts, emptySet()))
        assertFalse(shouldAutoResumeOnLaunch(true, last, listOf(host(8)), emptySet()))
        assertFalse(shouldAutoResumeOnLaunch(true, last, listOf(host(7, keyId = null)), emptySet()))
        assertFalse(shouldAutoResumeOnLaunch(true, last, hosts, setOf(7)))
    }

    @Test
    fun aTargetTheUserClosedIsNeverAutoResumedEvenIfTheProcessDiedWithOtherSessionsOpen() {
        val store = MemoryPrefStore()
        val memory = ReattachMemory(store)
        memory.remember(last)
        SessionMarker(store).onOpenSessions(true)
        memory.terminalClosed(7, pane) // The user closed it; another session keeps the marker set...
        val marker = SessionMarker(store)
        assertTrue(marker.diedWithSessions)
        // ...but there is nothing remembered to resume.
        assertFalse(shouldAutoResumeOnLaunch(marker.takeColdResume(), ReattachMemory(store).last.value, listOf(host(7)), emptySet()))
    }

    // --- process death: resume at once, with no tap beyond the fingerprint ----------------------

    private fun host(id: Long, keyId: String? = "k") = io.github.code_akram.or2.connection.testHost(id, "H$id", keyId = keyId)

    @Test
    fun aProcessThatDiedOnATerminalResumesAtOnceWhenItCanBeUnlocked() {
        assertTrue(shouldAutoResume(last, listOf(host(7)), connectedHosts = emptySet()))
        // Nothing remembered (the user ended it), a deleted host, or a host without a key: only the Resume card is left.
        assertFalse(shouldAutoResume(null, listOf(host(7)), emptySet()))
        assertFalse(shouldAutoResume(last, listOf(host(8)), emptySet()))
        assertFalse(shouldAutoResume(last, listOf(host(7, keyId = null)), emptySet()))
        // Already connected (it cannot be after a death, but a reopen is not a resume).
        assertFalse(shouldAutoResume(last, listOf(host(7)), setOf(7)))
    }

    @Test
    fun theAutoResumeAgreesWithTheDecisionThatOffersTheResumeCard() {
        // Whenever the decision is Resume for a host that has a key, the auto path would also resume.
        val hosts = listOf(host(7))
        assertEquals(Reattach.Resume(last), decideReattach(last, emptyList(), emptySet(), setOf(7)))
        assertTrue(shouldAutoResume(last, hosts, emptySet()))
    }
}
