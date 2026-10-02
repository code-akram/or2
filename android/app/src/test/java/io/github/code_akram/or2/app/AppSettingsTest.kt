package io.github.code_akram.or2.app

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class AppSettingsTest {
    @Test
    fun copyFromTheHostIsOnByDefaultAndRemembered() {
        val prefs = MemoryPrefStore()
        val settings = AppSettings(prefs)
        assertTrue(settings.copyFromHost.value)

        settings.setCopyFromHost(false)
        assertFalse(settings.copyFromHost.value)
        assertFalse(AppSettings(prefs).copyFromHost.value) // A new process reads it back.

        settings.setCopyFromHost(true)
        assertTrue(AppSettings(prefs).copyFromHost.value)
    }
}
