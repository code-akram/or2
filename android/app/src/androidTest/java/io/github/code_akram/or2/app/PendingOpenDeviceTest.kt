package io.github.code_akram.or2.app

import androidx.compose.runtime.MutableState
import androidx.compose.ui.test.junit4.StateRestorationTester
import androidx.compose.ui.test.junit4.createComposeRule
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.ffi.TerminalTransport
import io.github.code_akram.or2.notify.AgentPaneKey
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Rule
import org.junit.Test

/** A resume or an agent tap that is waiting on its connect keeps its target when the activity is recreated. */
class PendingOpenDeviceTest {
    @get:Rule val compose = createComposeRule()

    @Test
    fun thePendingOpenSurvivesRecreationAndIsClearedOnceOpened() {
        val restoration = StateRestorationTester(compose)
        var pending: MutableState<PendingOpen?>? = null
        restoration.setContent { pending = rememberPendingOpen() }
        val resume = PendingOpen.Resume(LastTerminal(7, TerminalTarget.Herdr("work", "w1:p2"), TerminalTransport.MOSH), started = true)
        compose.runOnIdle { pending!!.value = resume }

        restoration.emulateSavedInstanceStateRestore() // Rotation, or a restored process.
        compose.runOnIdle { assertEquals(resume, pending!!.value) }

        val agent = PendingOpen.Agent(AgentPaneKey(8, null, "w1:p1"))
        compose.runOnIdle { pending!!.value = agent } // A newer request replaces it.
        restoration.emulateSavedInstanceStateRestore()
        compose.runOnIdle { assertEquals(agent, pending!!.value) }

        compose.runOnIdle { pending!!.value = null } // Opened: the continuation is spent.
        restoration.emulateSavedInstanceStateRestore()
        compose.runOnIdle { assertNull(pending!!.value) }
    }
}
