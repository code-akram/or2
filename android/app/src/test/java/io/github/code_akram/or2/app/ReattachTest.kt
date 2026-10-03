package io.github.code_akram.or2.app

import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.ffi.TerminalTransport
import org.junit.Assert.*
import org.junit.Test

/** Reattach: what is remembered and what is decided on return. The one-time prompts are in [OneTimePromptsTest]. */
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
