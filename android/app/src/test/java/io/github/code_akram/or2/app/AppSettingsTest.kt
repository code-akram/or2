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

    @Test
    fun keepScreenOnIsOffByDefaultAndRemembered() {
        val prefs = MemoryPrefStore()
        val settings = AppSettings(prefs)
        assertFalse(settings.keepScreenOn.value)

        settings.setKeepScreenOn(true)
        assertTrue(settings.keepScreenOn.value)
        assertTrue(AppSettings(prefs).keepScreenOn.value)

        settings.setKeepScreenOn(false)
        assertFalse(AppSettings(prefs).keepScreenOn.value)
    }

    @Test
    fun reopeningTheLastTerminalIsOnByDefaultAndRemembered() {
        val prefs = MemoryPrefStore()
        val settings = AppSettings(prefs)
        assertTrue(settings.reopenLastTerminal.value)

        settings.setReopenLastTerminal(false)
        assertFalse(settings.reopenLastTerminal.value)
        assertFalse(AppSettings(prefs).reopenLastTerminal.value)

        settings.setReopenLastTerminal(true)
        assertTrue(AppSettings(prefs).reopenLastTerminal.value)
    }

    @Test
    fun theSettingsAreIndependent() {
        val prefs = MemoryPrefStore()
        val settings = AppSettings(prefs)
        settings.setKeepScreenOn(true)
        settings.setReopenLastTerminal(false)
        val read = AppSettings(prefs)
        assertTrue(read.copyFromHost.value)
        assertTrue(read.keepScreenOn.value)
        assertFalse(read.reopenLastTerminal.value)
    }
}
