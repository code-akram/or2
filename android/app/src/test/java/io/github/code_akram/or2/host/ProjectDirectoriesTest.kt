package io.github.code_akram.or2.host

import io.github.code_akram.or2.ffi.AgentStatus
import io.github.code_akram.or2.ffi.HerdrAgent
import io.github.code_akram.or2.ffi.HerdrPane
import io.github.code_akram.or2.ffi.HerdrTab
import io.github.code_akram.or2.ffi.HerdrView
import io.github.code_akram.or2.ffi.HerdrWorkspace
import io.github.code_akram.or2.ffi.mergeDirectoryPaths
import org.junit.Assert.assertTrue
import org.junit.Assert.assertEquals
import org.junit.Assert.assertSame
import org.junit.Test

/** Uses the real pure Rust FFI merge: no protocol, host, histories or persistence. */
class ProjectDirectoriesTest {
    private fun view(paneCwd: String?, agentCwd: String? = null, extra: List<HerdrPane> = emptyList()) = HerdrView(
        1uL, "w1:p1", listOf(HerdrWorkspace("w1", 1u, "Project")), listOf(HerdrTab("w1:t1", "w1", 1u, "pi")),
        listOf(HerdrPane("w1:p1", "pi", paneCwd)) + extra,
        listOf(HerdrAgent("w1:p1", "w1:t1", "w1", null, "pi", "pi", AgentStatus.IDLE, agentCwd, 1uL, "term_1")),
    )

    @Test
    fun livePiAndPlainShellDirectoriesNeedNoAgentHistory() {
        assertEquals(
            DirectoryList.Loaded(listOf("/live/pi", "/live/plain-shell")),
            projectDirectories(DirectoryList.Loaded(emptyList()), mapOf(null to view("/live/pi", extra = listOf(HerdrPane("w1:p2", null, "/live/plain-shell"))))),
        )
    }

    @Test
    fun defaultSessionThenNamedSessionsKeepPickerOrderAndPrecedeDeduplicatedHistory() {
        val named = view("/live/named")
        val default = view("/live/default-pane", "/live/default-agent", listOf(HerdrPane("w1:p2", null, "/live/plain")))
        assertEquals(
            DirectoryList.Loaded(listOf("/live/default-agent", "/live/default-pane", "/live/plain", "/live/named", "/history")),
            projectDirectories(DirectoryList.Loaded(listOf("/live/named", "/history", "/live/plain")), linkedMapOf("named" to named, null to default)),
        )
    }

    @Test
    fun liveRowsRemainUsableWhileHistoryIsLoadingOrFailed() {
        val views = mapOf<String?, HerdrView>(null to view("/live/pi"))
        val expected = DirectoryList.Loaded(listOf("/live/pi"))
        assertEquals(expected, projectDirectories(DirectoryList.Loading, views))
        assertEquals(expected, projectDirectories(DirectoryList.Failed("history timeout"), views))
    }

    @Test
    fun noUsefulLiveRowsPreserveTheHistoryLoadingOrErrorState() {
        val failed = DirectoryList.Failed("history timeout")
        assertSame(failed, projectDirectories(failed, emptyMap()))
        assertSame(DirectoryList.Loading, projectDirectories(DirectoryList.Loading, mapOf(null to view("~/relative"))))
    }

    @Test
    fun liveChangesAndUnavailableOrRemovedWatchesNeverLeaveCachedLivePaths() {
        val history = DirectoryList.Loaded(listOf("/history"))
        assertEquals(DirectoryList.Loaded(listOf("/live/first", "/history")), projectDirectories(history, mapOf(null to view("/live/first"))))
        assertEquals(DirectoryList.Loaded(listOf("/live/moved", "/history")), projectDirectories(history, mapOf(null to view("/live/moved"))))
        assertEquals(history, projectDirectories(history, emptyMap())) // Watch not Live, removed, or watching disabled.
    }

    @Test
    fun unsafeCwdAndHistoryAreRejectedWithoutSanitizingOrExpandingThem() {
        assertEquals(
            DirectoryList.Loaded(emptyList()),
            projectDirectories(DirectoryList.Loaded(listOf("/bad\npath", "/back\\slash", "relative")), mapOf(null to view("/hidden\u202epath", "~/project"))),
        )
    }

    @Test
    fun aMissingAgentCwdFallsBackToItsPaneBeforeTheNextAgent() {
        val base = view("/pane/first")
        val projected = base.copy(
            panes = base.panes + HerdrPane("w1:p2", "pi", "/pane/second"),
            agents = listOf(base.agents.single().copy(paneId = "w1:p2", cwd = "/agent/second"), base.agents.single()),
        )
        assertEquals(
            DirectoryList.Loaded(listOf("/pane/first", "/agent/second", "/pane/second")),
            projectDirectories(DirectoryList.Loaded(emptyList()), mapOf(null to projected)),
        )
    }

    @Test
    fun largeViewsUseBoundedFfiBatchesWithoutInvalidPathsCrowdingOutValidOnes() {
        val invalid = (0..79).map { HerdrPane("p${it.toString().padStart(3, '0')}", null, "/hidden\u202e$it") }
        val valid = (80..109).map { HerdrPane("p${it.toString().padStart(3, '0')}", null, "/live/$it") }
        val oversized = HerdrPane("p000-large", null, "/" + "x".repeat(4096))
        val projected = view(null, extra = invalid + valid + oversized)
        var calls = 0
        val result = projectDirectories(DirectoryList.Loaded(listOf("/history")), mapOf(null to projected)) { first, next ->
            calls++
            assertTrue(first.size <= 20)
            assertTrue(next.size <= 32)
            assertTrue(next.all { it.length <= 4096 })
            mergeDirectoryPaths(first, next)
        }
        assertTrue(calls >= 4)
        assertEquals(DirectoryList.Loaded((80..99).map { "/live/$it" }), result)
    }

    @Test
    fun oneSharedCapAppliesAfterValidationAndExactDeduplication() {
        val panes = (0..29).map { HerdrPane("w1:p${it.toString().padStart(2, '0')}", null, "/live/$it") }
        val projected = projectDirectories(DirectoryList.Loaded(listOf("/history")), mapOf(null to view(null, extra = panes)))
        assertEquals(DirectoryList.Loaded((0..19).map { "/live/$it" }), projected)
    }
}
