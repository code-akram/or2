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

    companion object {
        /** Stored inverted: an absent key reads false, which is the default (on). */
        const val HOST_COPY_OFF = "host_copy_off"
    }
}
