package io.github.code_akram.or2.home

import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.HostEndpoint
import io.github.code_akram.or2.data.HostRecord
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.PublicKeyInfo
import io.github.code_akram.or2.ffi.SessionFailure
import io.github.code_akram.or2.session.hostStateMessage
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class HomeModelTest {
    @Test
    fun connectionProgressReplacesTheAddressInPlaceWithASpinner() {
        val unlocking = hostCardStatus(null, unlocking = true, blockedAgents = 0)
        assertEquals("Unlocking key…", unlocking.progress)
        assertEquals(HostDot.CONNECTING, unlocking.dot)
        assertTrue(unlocking.spinning)
        val checking = hostCardStatus(HostState.Connecting, unlocking = false, blockedAgents = 0)
        assertEquals("Checking server…", checking.progress)
        assertTrue(checking.spinning)
        val authenticating = hostCardStatus(HostState.Authenticating, unlocking = true, blockedAgents = 0)
        assertEquals("Authenticating…", authenticating.progress) // A live connection outranks "unlocking".
        assertTrue(authenticating.spinning)
    }

    @Test
    fun aHostKeyDecisionNeedsAttentionWithoutASpinner() {
        val prompt = HostState.AwaitingHostKeyDecision(PublicKeyInfo("ssh-ed25519", "k", "SHA256:x", ""), emptyList())
        val status = hostCardStatus(prompt, unlocking = false, blockedAgents = 0)
        assertEquals("Waiting for host-key approval", status.progress)
        assertEquals(HostDot.ATTENTION, status.dot)
        assertFalse(status.spinning)
    }

    @Test
    fun connectedHostsShowTheirAddressAndAnAttentionDotOnlyWhenAnAgentIsBlocked() {
        val quiet = hostCardStatus(HostState.Connected(0u), unlocking = false, blockedAgents = 0)
        assertNull(quiet.progress)
        assertNull(quiet.failure)
        assertEquals(HostDot.CONNECTED, quiet.dot)
        assertEquals(HostDot.ATTENTION, hostCardStatus(HostState.Connected(0u), unlocking = false, blockedAgents = 2).dot)
    }

    @Test
    fun failuresExplainThemselvesAndClosedHostsAreQuiet() {
        val failed = hostCardStatus(HostState.Closed(CloseReason.Failed(SessionFailure.AuthenticationRejected)), unlocking = false, blockedAgents = 0)
        assertEquals(HostDot.FAILED, failed.dot)
        assertTrue(failed.failure!!.contains("Authentication rejected"))
        assertNull(failed.progress)
        // A retry in progress shows the progress, not the old failure.
        assertEquals("Unlocking key…", hostCardStatus(HostState.Closed(CloseReason.Failed(SessionFailure.TimedOut)), true, 0).progress)
        for (state in listOf(null, HostState.Closed(CloseReason.Disconnected))) {
            assertEquals(HostCardStatus(null, null, HostDot.NONE), hostCardStatus(state, unlocking = false, blockedAgents = 3))
        }
    }

    @Test
    fun theAddressLineIsMonoUserAtHostPortWithTheCountOfOthers() {
        fun host(vararg endpoints: HostEndpoint) = Host(HostRecord(1, "Box", "dev", null), endpoints.toList())
        assertEquals("dev@workstation.invalid:22", hostAddressLine(host(HostEndpoint("workstation.invalid", 22))))
        assertEquals("dev@a.invalid:2222 +2", hostAddressLine(host(HostEndpoint("a.invalid", 2222), HostEndpoint("b.invalid", 22), HostEndpoint("c.invalid", 22))))
    }

    @Test
    fun aSleepingHostsLostConnectionIsMutedAsleepWithNoFailureAndNoDot() {
        val lost = HostState.Closed(CloseReason.Failed(SessionFailure.ConnectionLost("reset")))
        assertEquals(HostCardStatus(null, hostStateMessage(lost), HostDot.FAILED), hostCardStatus(lost, unlocking = false, blockedAgents = 0))
        val asleep = hostCardStatus(lost, unlocking = false, blockedAgents = 0, sleeps = true)
        assertEquals(HostCardStatus(null, null, HostDot.NONE, asleep = true), asleep)
        assertNull(asleep.failure)
        // Unlocking still shows its progress, and a connected host is just connected.
        assertEquals("Unlocking key\u2026", hostCardStatus(lost, unlocking = true, blockedAgents = 0, sleeps = true).progress)
        assertEquals(HostDot.CONNECTED, hostCardStatus(HostState.Connected(0u), unlocking = false, blockedAgents = 0, sleeps = true).dot)
    }

    private val twoAddresses = listOf(HostEndpoint("workstation.local", 22), HostEndpoint("198.51.100.20", 22))
    private val unreachable = HostState.Closed(
        CloseReason.Failed(
            SessionFailure.Unreachable("TCP connection failed: address 0: name not resolved (mDNS) after 3 tries; address 1: no answer within 6 s"),
        ),
    )

    @Test
    fun anUnreachableHostExplainsWhatEachAddressDidInMutedLines() {
        val status = hostCardStatus(unreachable, unlocking = false, blockedAgents = 0, addresses = twoAddresses)
        assertEquals(HostDot.FAILED, status.dot)
        assertEquals(
            "workstation.local:22 \u00b7 name not resolved (mDNS) after 3 tries\n198.51.100.20:22 \u00b7 no answer within 6 s",
            status.detail,
        )
        // Without the host's addresses (or for another failure) there is nothing to add.
        assertNull(hostCardStatus(unreachable, false, 0).detail)
        assertNull(hostCardStatus(HostState.Closed(CloseReason.Failed(SessionFailure.TimedOut)), false, 0, addresses = twoAddresses).detail)
    }

    @Test
    fun aSleepingHostThatTimedOutReadsAsleepNotAsAnErrorAndKeepsTheExplanation() {
        val timedOut = HostState.Closed(CloseReason.Failed(SessionFailure.TimedOut))
        val asleep = hostCardStatus(timedOut, unlocking = false, blockedAgents = 0, sleeps = true, addresses = twoAddresses)
        assertTrue(asleep.asleep)
        assertNull(asleep.failure)
        assertEquals(HostDot.NONE, asleep.dot)
        // The same for an unreachable one: muted, with what each address did underneath.
        val quiet = hostCardStatus(unreachable, unlocking = false, blockedAgents = 0, sleeps = true, addresses = twoAddresses)
        assertTrue(quiet.asleep)
        assertNull(quiet.failure)
        assertTrue(quiet.detail!!.contains("workstation.local:22"))
        // A rejected key is no sleep, whatever the flag says.
        val rejected = hostCardStatus(HostState.Closed(CloseReason.Failed(SessionFailure.AuthenticationRejected)), false, 0, sleeps = true)
        assertFalse(rejected.asleep)
        assertNotNull(rejected.failure)
    }
}
