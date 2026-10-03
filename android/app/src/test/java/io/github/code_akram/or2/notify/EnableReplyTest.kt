package io.github.code_akram.or2.notify

import io.github.code_akram.or2.ffi.AgentIdentity
import io.github.code_akram.or2.ffi.AgentSession
import io.github.code_akram.or2.ffi.AgentStatus
import io.github.code_akram.or2.ffi.HerdrAgent
import io.github.code_akram.or2.ffi.HerdrIntegrationState
import io.github.code_akram.or2.ffi.HerdrView
import io.github.code_akram.or2.ffi.HostException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Enable Reply (contracts.md, "Lane App: Enable Reply from the phone"): which agents are offered it, and what it says. */
@OptIn(ExperimentalCoroutinesApi::class)
class EnableReplyTest {
    /** An agent of [kind]; with [session], herdr reports its session (it has Reply). */
    private fun agent(kind: String?, session: String? = null, pane: String = "w1:p1", status: AgentStatus = AgentStatus.BLOCKED, seq: ULong = 1u) =
        HerdrAgent(
            pane, "w1:t1", "w1", null, kind, kind, status, "/work", seq, "term_$pane",
            session?.let { AgentIdentity("term_$pane", kind, null, AgentSession("id", it)) },
        )

    private val status = mapOf(
        "pi" to HerdrIntegrationState.NOT_INSTALLED,
        "opencode" to HerdrIntegrationState.OUTDATED,
        "codex" to HerdrIntegrationState.CURRENT,
        "claude" to HerdrIntegrationState.CURRENT,
        "cursor" to HerdrIntegrationState.NOT_INSTALLED,
        "antigravity-cli" to HerdrIntegrationState.NOT_INSTALLED,
    )

    @Test
    fun everyKindHerdrHasAnIntegrationForMapsToItsIdAndNoOtherDoes() {
        for (id in HerdrIntegrations.IDS) assertEquals(id, HerdrIntegrations.forKind(id))
        assertEquals(18, HerdrIntegrations.IDS.size)
        // Kinds named after the executable.
        assertEquals("cursor", HerdrIntegrations.forKind("cursor-agent"))
        assertEquals("antigravity-cli", HerdrIntegrations.forKind("agy"))
        assertEquals("antigravity-cli", HerdrIntegrations.forKind("antigravity_cli"))
        assertEquals("pi", HerdrIntegrations.forKind(" Pi "))
        // Kinds without an integration, and none at all.
        for (kind in listOf("amp", "gemini", "cline", "aider", "", " ", null, "pi;rm", "claude code")) {
            assertNull(kind, HerdrIntegrations.forKind(kind))
        }
    }

    @Test
    fun onlyAnAgentWithoutASessionWhoseIntegrationIsMissingOrOutdatedIsOffered() {
        // No session, integration not installed or outdated: offered.
        assertEquals("pi", enableReplyFor(agent("pi"), status))
        assertEquals("opencode", enableReplyFor(agent("opencode"), status))
        assertEquals("cursor", enableReplyFor(agent("cursor-agent"), status))
        assertEquals("antigravity-cli", enableReplyFor(agent("agy"), status))
        // It has Reply already: nothing new.
        assertNull(enableReplyFor(agent("pi", session = "sess_1"), status))
        // Installed but still no session (Codex with herdr 0.9.3; an agent started before the install): nothing new.
        assertNull(enableReplyFor(agent("codex"), status))
        assertNull(enableReplyFor(agent("claude"), status))
        // A kind without an integration, or no kind: nothing new.
        assertNull(enableReplyFor(agent("amp"), status))
        assertNull(enableReplyFor(agent(null), status))
        // The host's integrations are not known (not read yet, or herdr could not say), or this herdr lists none for it.
        assertNull(enableReplyFor(agent("pi"), null))
        assertNull(enableReplyFor(agent("droid"), status))
    }

    @Test
    fun theConfirmationAndTheOutcomesSayWhatTheContractSays() {
        val request = EnableReplyRequest(1, "archlinux", "pi", "pi")
        assertEquals(
            "Enable Reply for pi on archlinux? or2 installs herdr's pi integration there. Restart pi afterwards.",
            request.question,
        )
        assertEquals("Done. Restart pi to reply to it.", request.done)
        // An agent herdr names otherwise than its integration.
        val claude = EnableReplyRequest(1, "archlinux", "Claude Code", "claude")
        assertEquals(
            "Enable Reply for Claude Code on archlinux? or2 installs herdr's claude integration there. Restart Claude Code afterwards.",
            claude.question,
        )
        assertEquals("Done. Restart Claude Code to reply to it.", claude.done)
    }

    @Test
    fun anInstallSaysDoneOrWhyNot() = runTest {
        val request = EnableReplyRequest(7, "archlinux", "pi", "pi")
        val asked = mutableListOf<Pair<Long, String>>()
        assertEquals(request.done, enableReplyOutcome(request) { hostId, id -> asked += hostId to id })
        assertEquals(listOf(7L to "pi"), asked)
        suspend fun failing(error: HostException) = enableReplyOutcome(request) { _, _ -> throw error }
        assertEquals("Not enabled: archlinux is not connected", failing(HostException.NotConnected()))
        assertEquals("Not enabled: archlinux is not connected", failing(HostException.Closed()))
        assertEquals("Not enabled: herdr is not installed on archlinux", failing(HostException.NotInstalled("herdr")))
        assertEquals(
            "Not enabled: error: cannot write the hook: permission denied",
            failing(HostException.CommandFailed("error: cannot write the hook: permission denied")),
        )
        assertEquals("Not enabled: " + "x".repeat(80), failing(HostException.CommandFailed("x".repeat(200))))
        assertEquals("Not enabled: herdr has no such integration", failing(HostException.InvalidName()))
        // A host that never answers: Kotlin's bound, above Rust's.
        val never = CompletableDeferred<Unit>()
        assertEquals(NOT_ENABLED_TIMEOUT, enableReplyOutcome(request, timeoutMs = 1_000) { _, _ -> never.await() })
    }

    @Test
    fun oneRequestWaitsAtATimeAndIsTakenOnce() {
        val requests = EnableReplyRequests()
        val first = EnableReplyRequest(1, "archlinux", "pi", "pi")
        val second = EnableReplyRequest(2, "build-box", "opencode", "opencode")
        requests.request(first)
        requests.request(second)
        assertEquals(second, requests.request.value)
        assertEquals(second, requests.take())
        assertNull(requests.take())
    }

    @Test
    fun aNotificationForAnAgentWithoutReplyOffersEnableReplyAndOnlyThen() {
        val sink = RecordingSink()
        val alerts = AgentAlerts(sink)
        val offered = mutableListOf<String?>()
        alerts.enableReply = { hostId, agent -> assertEquals(1L, hostId); enableReplyFor(agent, status).also { offered += it } }
        fun deliver(vararg agents: HerdrAgent) =
            alerts.viewChanged(this, 1, "archlinux", null, HerdrView(1uL, null, emptyList(), emptyList(), emptyList(), agents.toList()))
        val panes = listOf("pi" to null, "claude" to "sess_2", "codex" to null, "amp" to null)
        fun all(status: AgentStatus, seq: ULong) = panes.mapIndexed { i, (kind, session) ->
            agent(kind, session, pane = "w1:p$i", status = status, seq = seq)
        }.toTypedArray()
        deliver(*all(AgentStatus.WORKING, 1u))
        deliver(*all(AgentStatus.BLOCKED, 2u))
        val byPane = sink.posted.associateBy { it.key.paneId }
        // pi: no session, its integration missing: Enable Reply, which asks for pi on archlinux.
        assertEquals("pi", byPane.getValue("w1:p0").enableReply)
        assertEquals(EnableReplyRequest(1, "archlinux", "pi", "pi"), byPane.getValue("w1:p0").enableReplyRequest)
        // claude has Reply (the lookup is not even asked); codex's integration is current; amp has none.
        assertNull(byPane.getValue("w1:p1").enableReply)
        assertTrue(byPane.getValue("w1:p1").agent != null)
        assertNull(byPane.getValue("w1:p1").enableReplyRequest)
        assertNull(byPane.getValue("w1:p2").enableReply)
        assertNull(byPane.getValue("w1:p3").enableReply)
        assertEquals(listOf("pi", null, null), offered)
    }

    @Test
    fun anEnableReplyFromTheNotificationIsTakenOnceAndTheNotificationGoes() {
        val sink = RecordingSink()
        val nonces = ReplyNonces(io.github.code_akram.or2.app.MemoryPrefStore())
        val alerts = AgentAlerts(sink, nonces)
        alerts.enableReply = { _, agent -> enableReplyFor(agent, status) }
        fun deliver(status: AgentStatus, seq: ULong) = alerts.viewChanged(
            this, 1, "archlinux", null, HerdrView(1uL, null, emptyList(), emptyList(), emptyList(), listOf(agent("pi", status = status, seq = seq))),
        )
        deliver(AgentStatus.WORKING, 1u)
        deliver(AgentStatus.BLOCKED, 2u)
        val posted = sink.posted.single()
        val key = posted.key
        // Another post's capability, or none, is refused.
        assertEquals(false, alerts.admitEnableReply(key, "stale"))
        assertEquals(false, alerts.admitEnableReply(key, null))
        assertTrue(alerts.admitEnableReply(key, posted.nonce))
        // Taken: the notification went, and the same capability is refused from now on.
        assertTrue(key !in sink.up)
        assertEquals(false, alerts.admitEnableReply(key, posted.nonce))
    }

    @Test
    fun anEnableReplyIntentIsTakenOnlyWithTheAppsTokenAPaneAndAnAllowlistedIntegration() {
        val tag = AgentPaneKey(3, "work", "w1:p2").tag
        val ask = enableReplyFrom(tag, "n1", "pi", "pi", "archlinux", "token", "token")
        assertEquals(EnableReplyAsk(AgentPaneKey(3, "work", "w1:p2"), "n1", EnableReplyRequest(3, "archlinux", "pi", "pi")), ask)
        assertNull(enableReplyFrom(tag, "n1", "pi", "pi", "archlinux", "other", "token"))
        assertNull(enableReplyFrom(tag, "n1", "pi", "pi", "archlinux", null, null))
        assertNull(enableReplyFrom("not-a-tag", "n1", "pi", "pi", "archlinux", "token", "token"))
        assertNull(enableReplyFrom(tag, "n1", "amp", "amp", "archlinux", "token", "token"))
        assertNull(enableReplyFrom(tag, "n1", "pi; reboot", "pi", "archlinux", "token", "token"))
        assertNull(enableReplyFrom(tag, "n1", null, "pi", "archlinux", "token", "token"))
        // No title: the agent is still named.
        assertEquals("agent", enableReplyFrom(tag, "n1", "pi", null, "archlinux", "token", "token")?.request?.agent)
    }
}
