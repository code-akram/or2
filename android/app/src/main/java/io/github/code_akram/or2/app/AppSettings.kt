package io.github.code_akram.or2.app

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * The user's settings (the Settings screen). Every default is the zero-configuration choice, so a
 * setting is stored only once it is turned away from its default.
 */
class AppSettings(private val prefs: PrefStore) {
    private val mutableCopyFromHost = MutableStateFlow(!prefs.getBoolean(HOST_COPY_OFF))

    /** Programs on a host may set the phone's clipboard (OSC 52, OSC 1337 Copy). On by default. */
    val copyFromHost: StateFlow<Boolean> = mutableCopyFromHost.asStateFlow()

    fun setCopyFromHost(on: Boolean) {
        prefs.putBoolean(HOST_COPY_OFF, !on)
        mutableCopyFromHost.value = on
    }

    private val mutableKeepScreenOn = MutableStateFlow(prefs.getBoolean(KEEP_SCREEN_ON))

    /** The screen stays on while a terminal is on it (the window's keep-screen-on flag, nowhere else). Off by default. */
    val keepScreenOn: StateFlow<Boolean> = mutableKeepScreenOn.asStateFlow()

    fun setKeepScreenOn(on: Boolean) {
        prefs.putBoolean(KEEP_SCREEN_ON, on)
        mutableKeepScreenOn.value = on
    }

    private val mutableReopenLastTerminal = MutableStateFlow(!prefs.getBoolean(REOPEN_LAST_OFF))

    /**
     * A launch after the app was killed with a terminal open goes straight back to it (the existing reattach and resume
     * on launch). On by default; off leaves the app on Home with its Resume card, and changes nothing else.
     */
    val reopenLastTerminal: StateFlow<Boolean> = mutableReopenLastTerminal.asStateFlow()

    fun setReopenLastTerminal(on: Boolean) {
        prefs.putBoolean(REOPEN_LAST_OFF, !on)
        mutableReopenLastTerminal.value = on
    }

    companion object {
        /** Stored inverted: an absent key reads false, which is the default (on). */
        const val HOST_COPY_OFF = "host_copy_off"

        /** Off by default, so stored as it reads. */
        const val KEEP_SCREEN_ON = "keep_screen_on"

        /** Stored inverted, like [HOST_COPY_OFF]. */
        const val REOPEN_LAST_OFF = "reopen_last_off"
    }
}
