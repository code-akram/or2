package io.github.code_akram.or2

import io.github.code_akram.or2.ffi.Renderer
import io.github.code_akram.or2.ffi.TerminalException
import io.github.code_akram.or2.ffi.buildInfo
import io.github.code_akram.or2.ffi.terminalSize
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class NativeContractTest {
    @Test
    fun loadsRustAndReportsTheScaffoldContract() {
        val info = buildInfo()
        assertEquals("0.1.0", info.version)
        assertEquals(11u, info.apiVersion)
        assertEquals(34u, info.minimumAndroidSdk)
        assertEquals(Renderer.CANVAS, info.renderer)
    }

    @Test
    fun preservesUnsignedGeometryAndMapsRustErrors() {
        val size = terminalSize(97u, 31u)
        assertEquals(97.toUShort(), size.columns)
        assertEquals(31.toUShort(), size.rows)
        assertEquals(3007u, size.cellCount)
        assertEquals(4294836225u, terminalSize(65535u, 65535u).cellCount)
        assertEquals(1u, terminalSize(1u, 1u).cellCount)
        assertThrows(TerminalException.EmptyDimension::class.java) { terminalSize(0u, 31u) }
        assertThrows(TerminalException.EmptyDimension::class.java) { terminalSize(97u, 0u) }
    }
}
