package io.github.code_akram.or2.host

import io.github.code_akram.or2.ffi.HerdrSessionInfo
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.ffi.TmuxSession
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** The picker marks a herdr or tmux session `● Open` when it already has an open terminal in or2. */
class OpenSessionsTest {
    private fun herdr(name: String, default: Boolean = false) = HerdrSessionInfo(name, true, default)
    private fun tmux(name: String) = TmuxSession(name, 1u, 0u, 0L, 0L)

    @Test
    fun aSessionWithAnOpenTerminalIsMarkedWhateverPaneItWasOpenedOn() {
        val open = OpenSessions.of(listOf(
            TerminalTarget.Herdr("work", "w1:p3"), // An agent's terminal is its session's terminal too.
            TerminalTarget.Tmux("main"),
            TerminalTarget.Shell,
        ))
        assertTrue(open.has(herdr("work")))
        assertFalse(open.has(herdr("personal")))
        assertTrue(open.has(tmux("main")))
        assertFalse(open.has(tmux("build")))
        // A shell is no session: nothing else is marked.
        assertFalse(open.has(herdr("default", default = true)))
    }

    @Test
    fun herdrsDefaultSessionIsMarkedByNoNameOrByItsListedName() {
        // The picker opens the default session without a name; an agent's terminal may name it.
        assertTrue(OpenSessions.of(listOf(TerminalTarget.Herdr(null, null))).has(herdr("personal", default = true)))
        assertTrue(OpenSessions.of(listOf(TerminalTarget.Herdr("personal", "w1:p1"))).has(herdr("personal", default = true)))
        // A named session is not the default one.
        assertFalse(OpenSessions.of(listOf(TerminalTarget.Herdr(null, null))).has(herdr("work")))
    }

    @Test
    fun nothingOpenMarksNothing() {
        val open = OpenSessions.of(emptyList())
        assertFalse(open.has(herdr("work")))
        assertFalse(open.has(herdr("personal", default = true)))
        assertFalse(open.has(tmux("main")))
    }
}
