package io.github.code_akram.or2.app

import androidx.compose.runtime.MutableState
import androidx.compose.ui.test.junit4.StateRestorationTester
import androidx.compose.ui.test.junit4.createComposeRule
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.ffi.TerminalTransport
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Rule
import org.junit.Test

/** A cold resume that is waiting on its connect keeps its target when the activity is recreated. */
class PendingResumeDeviceTest {
    @get:Rule val compose = createComposeRule()

    @Test
    fun theResumeTargetSurvivesRecreationAndIsClearedOnceReopened() {
        val restoration = StateRestorationTester(compose)
        var pending: MutableState<LastTerminal?>? = null
        restoration.setContent { pending = rememberPendingResume() }
        val target = LastTerminal(7, TerminalTarget.Herdr("work", "w1:p2"), TerminalTransport.MOSH)
        compose.runOnIdle { pending!!.value = target }

        restoration.emulateSavedInstanceStateRestore() // Rotation, or a restored process.
        compose.runOnIdle { assertEquals(target, pending!!.value) }

        compose.runOnIdle { pending!!.value = null } // Reopened: the continuation is spent.
        restoration.emulateSavedInstanceStateRestore()
        compose.runOnIdle { assertNull(pending!!.value) }
    }
}
