package io.github.code_akram.or2.connection

import io.github.code_akram.or2.data.TransportPref
import io.github.code_akram.or2.ffi.LinkHealth
import io.github.code_akram.or2.ffi.SessionFailure
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.ffi.TerminalTransport
import org.junit.Assert.*
import org.junit.Test

class TransportChoiceTest {
    private val found = MoshServerAnswer("/usr/bin/mosh-server", roundTripMs = 40)
    private val absent = MoshServerAnswer(null, roundTripMs = 40)
    private val shell = TerminalTarget.Shell
    private val swappable = listOf(TerminalTarget.Tmux("main"), TerminalTarget.Herdr(null, "w1:p1"), TerminalTarget.Herdr("work", null))
    private val ssh = OpenPlan(TerminalTransport.SSH)

    @Test
    fun autoWithUdpUntestedOpensTmuxAndHerdrOverSshAtOnceWithAMoshTryBehind() {
        for (target in swappable) {
            // Whether or not the probe has answered: nothing waits.
            assertEquals(OpenPlan(TerminalTransport.SSH, background = true), planOpen(TransportPref.AUTO, target, UdpVerdict.UNKNOWN, found))
            assertEquals(OpenPlan(TerminalTransport.SSH, background = true), planOpen(TransportPref.AUTO, target, UdpVerdict.UNKNOWN, null))
        }
    }

    @Test
    fun autoWithUdpKnownToWorkOpensMoshDirectlyWithTheFallback() {
        for (target in swappable + shell) {
            assertEquals(OpenPlan(TerminalTransport.MOSH, AUTO_MOSH_BUDGET_MS, fallbackEligible = true),
                planOpen(TransportPref.AUTO, target, UdpVerdict.OK, found))
        }
        assertEquals(5_000u, AUTO_MOSH_BUDGET_MS)
    }

    @Test
    fun autoWithUdpBlockedOrNoMoshServerOpensSshWithNoAttempt() {
        for (target in swappable + shell) {
            assertEquals(ssh, planOpen(TransportPref.AUTO, target, UdpVerdict.BLOCKED, found))
            assertEquals(ssh, planOpen(TransportPref.AUTO, target, UdpVerdict.BLOCKED, null))
            assertEquals(ssh, planOpen(TransportPref.AUTO, target, UdpVerdict.UNKNOWN, absent))
            assertEquals(ssh, planOpen(TransportPref.AUTO, target, UdpVerdict.OK, absent))
        }
    }

    @Test
    fun autoShellWithUdpUntestedTriesMoshOnABudgetFromTheProbesRoundTrip() {
        // Two shells cannot be swapped: mosh first, on a short budget, then SSH.
        assertEquals(OpenPlan(TerminalTransport.MOSH, 700u, fallbackEligible = true), planOpen(TransportPref.AUTO, shell, UdpVerdict.UNKNOWN, found))
        val slow = MoshServerAnswer("/usr/bin/mosh-server", roundTripMs = 300)
        assertEquals(OpenPlan(TerminalTransport.MOSH, 1_800u, fallbackEligible = true), planOpen(TransportPref.AUTO, shell, UdpVerdict.UNKNOWN, slow))
        // The probe has not answered: no mosh-server to go on, and no wait for it.
        assertEquals(ssh, planOpen(TransportPref.AUTO, shell, UdpVerdict.UNKNOWN, null))
    }

    @Test
    fun theShellBudgetIsSixRoundTripsWithAFloorAndACeiling() {
        assertEquals(700u, shellMoshBudgetMs(0))
        assertEquals(700u, shellMoshBudgetMs(116))
        assertEquals(702u, shellMoshBudgetMs(117))
        assertEquals(1_200u, shellMoshBudgetMs(200))
        assertEquals(15_000u, shellMoshBudgetMs(10_000))
    }

    @Test
    fun explicitChoicesAreHonouredWhateverTheProbeAndTheVerdictSay() {
        for (verdict in UdpVerdict.entries) for (answer in listOf(found, absent, null)) for (target in swappable + shell) {
            assertEquals(ssh, planOpen(TransportPref.SSH, target, verdict, answer))
            assertEquals(OpenPlan(TerminalTransport.MOSH), planOpen(TransportPref.MOSH, target, verdict, answer))
        }
    }

    @Test
    fun onlyTimedOutAndNotInstalledFallBack() {
        assertTrue(isMoshFallback(SessionFailure.TimedOut))
        assertTrue(isMoshFallback(SessionFailure.NotInstalled("mosh-server")))
        // A missing tmux or herdr is not mosh's problem: the SSH retry would fail the same way.
        assertFalse(isMoshFallback(SessionFailure.NotInstalled("tmux")))
        assertFalse(isMoshFallback(SessionFailure.NotInstalled("herdr")))
        assertFalse(isMoshFallback(SessionFailure.Unreachable("x")))
        assertFalse(isMoshFallback(SessionFailure.AuthenticationRejected))
        assertFalse(isMoshFallback(SessionFailure.ConnectionLost("x")))
        assertFalse(isMoshFallback(SessionFailure.CommandFailed("x")))
        assertFalse(isMoshFallback(SessionFailure.Internal("x")))
    }

    @Test
    fun theBlockedLineSaysWhyAndWhereTheFixIs() {
        // Only what is known: a firewall is one cause among several, so no fix is suggested.
        assertEquals("Mosh can't reach this host over UDP, so terminals use SSH.", UDP_BLOCKED_LINE)
    }

    @Test
    fun linkHealthGreysOnlyPastFiveSeconds() {
        assertNull(linkStaleLabel(null))
        assertNull(linkStaleLabel(LinkHealth(300uL, 300uL)))
        assertNull(linkStaleLabel(LinkHealth(5000uL, 9000uL))) // Exactly five seconds is still fine.
        assertEquals("Last heard 5 s ago", linkStaleLabel(LinkHealth(5001uL, 5001uL)))
        assertEquals("Last heard 12 s ago", linkStaleLabel(LinkHealth(12_400uL, 20_000uL)))
        assertNull(linkStaleLabel(LinkHealth(400uL, 400uL))) // Recovered.
    }
}
