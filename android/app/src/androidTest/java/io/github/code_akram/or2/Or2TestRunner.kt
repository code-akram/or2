package io.github.code_akram.or2

import androidx.test.runner.AndroidJUnitRunner
import io.github.code_akram.or2.terminal.TerminalPrefs

/**
 * Runs every device test against a scratch terminal-preferences file: a [io.github.code_akram.or2.terminal.TerminalView]
 * reads the saved font size when it is built, so grid sizes and layouts must not depend on the
 * owner's pinch setting, and a test that pinches must not write it. The instrumentation shares the
 * app's process, so the redirect covers all of them (and the debug probe activities) at once.
 */
class Or2TestRunner : AndroidJUnitRunner() {
    override fun onStart() {
        val previous = TerminalPrefs.file
        TerminalPrefs.file = SCRATCH_FILE
        targetContext.deleteSharedPreferences(SCRATCH_FILE)
        try {
            super.onStart()
        } finally {
            targetContext.deleteSharedPreferences(SCRATCH_FILE)
            TerminalPrefs.file = previous
        }
    }

    companion object {
        const val SCRATCH_FILE = "or2-terminal-test"
    }
}
