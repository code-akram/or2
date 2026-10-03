package io.github.code_akram.or2.home

import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.HostEndpoint
import io.github.code_akram.or2.data.HostRecord
import io.github.code_akram.or2.ffi.AgentStatus
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.HerdrAgent
import io.github.code_akram.or2.ffi.HerdrPane
import io.github.code_akram.or2.ffi.HerdrView
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.PublicKeyInfo
import io.github.code_akram.or2.ffi.SessionFailure
import io.github.code_akram.or2.inbox.LinkStatus
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
    fun aConnectedHostShowsTheAddressInUseWithTheCountOfOthers() {
        val host = Host(HostRecord(1, "Box", "dev", null), listOf(HostEndpoint("workstation.local", 22), HostEndpoint("198.51.100.7", 2222)))
        assertEquals("dev@198.51.100.7:2222 +1", hostAddressLine(host, HostState.Connected(1u)))
        assertEquals("dev@workstation.local:22 +1", hostAddressLine(host, HostState.Connected(0u)))
        // Not connected (or connecting, or closed): the first address, as before.
        assertEquals("dev@workstation.local:22 +1", hostAddressLine(host, null))
        assertEquals("dev@workstation.local:22 +1", hostAddressLine(host, HostState.Connecting))
        assertEquals("dev@workstation.local:22 +1", hostAddressLine(host, HostState.Closed(CloseReason.Disconnected)))
        // An index the host no longer has (its addresses were edited) falls back to the first.
        assertEquals("dev@workstation.local:22 +1", hostAddressLine(host, HostState.Connected(5u)))
        assertEquals("dev@198.51.100.7:2222 +1", HostCard(host, hostCardStatus(null, false, 0), LinkStatus.CONNECTED, hostAddressLine(host, HostState.Connected(1u))).address)
        assertEquals("dev@workstation.local:22 +1", HostCard(host, hostCardStatus(null, false, 0), LinkStatus.NOT_CONNECTED).address)
    }

    private fun pane(id: String, agent: String? = null, cwd: String? = null) =
        HerdrPane(id, agent, cwd)

    private fun agent(pane: String, name: String?, display: String? = name, cwd: String? = null) =
        HerdrAgent(pane, "w1:t1", "w1", name, name?.lowercase(), display, AgentStatus.WORKING, cwd, 1u, "term_$pane", null)

    private fun view(focused: String?, panes: List<HerdrPane>, agents: List<HerdrAgent> = emptyList()) =
        HerdrView(1u, focused, emptyList(), emptyList(), panes, agents)

    @Test
    fun aSessionCardShowsItsWorkingDirectoryElseWhatItsHerdrSessionShowsElseNothing() {
        val withAgent = view("w1:p2", listOf(pane("w1:p1", cwd = "~/one"), pane("w1:p2", "claude", "~/two")), listOf(agent("w1:p2", "Claude Code")))
        // The pane's own working directory comes first.
        assertEquals("~/src", sessionDetail("~/src", withAgent))
        // Else the focused pane's agent label.
        assertEquals("Claude Code", sessionDetail(null, withAgent))
        // An agent with no name of its own: the pane's agent kind.
        assertEquals("codex", sessionDetail(null, view("w1:p1", listOf(pane("w1:p1", "codex", "~/one")), listOf(agent("w1:p1", null)))))
        // No agent in the focused pane: its cwd.
        assertEquals("~/one", sessionDetail(null, view("w1:p1", listOf(pane("w1:p1", cwd = "~/one"), pane("w1:p2", "claude")))))
        // Nothing known: empty, never user@host (the card keeps its height).
        assertEquals("", sessionDetail(null, view("w1:p1", listOf(pane("w1:p1")))))
        assertEquals("", sessionDetail(null, view(null, listOf(pane("w1:p1", cwd = "~/one")))))
        assertEquals("", sessionDetail(null, null))
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

    @Test
    fun aTapOrTheSessionButtonConnectsOnlyAHostThatHasAKeyAndIsNotAlreadyOnItsWay() {
        val keyed = Host(HostRecord(1, "Box", "dev", "k"), listOf(HostEndpoint("box.invalid", 22)))
        val keyless = Host(HostRecord(2, "Bare", "dev", null), listOf(HostEndpoint("bare.invalid", 22)))
        assertTrue(tapConnects(keyed, LinkStatus.NOT_CONNECTED, busy = false))
        assertTrue(tapConnects(keyed, LinkStatus.FAILED, busy = false))
        assertTrue(tapConnects(keyed, LinkStatus.ASLEEP, busy = false)) // The user knows it woke up.
        assertFalse(tapConnects(keyed, LinkStatus.CONNECTED, busy = false))
        assertFalse(tapConnects(keyed, LinkStatus.CONNECTING, busy = false))
        assertFalse(tapConnects(keyed, LinkStatus.NEEDS_HOST_KEY, busy = false))
        assertFalse(tapConnects(keyed, LinkStatus.NOT_CONNECTED, busy = true)) // Another unlock is running.
        assertFalse(tapConnects(keyless, LinkStatus.NOT_CONNECTED, busy = false))
    }
}
