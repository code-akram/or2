package io.github.code_akram.or2

import io.github.code_akram.or2.ffi.buildInfo
import org.junit.Assert.assertEquals
import org.junit.Test

class NativeContractTest {
    @Test
    fun loadsRustAndReportsTheScaffoldContract() {
        val info = buildInfo()
        assertEquals("0.1.2", info.version)
        assertEquals(19u, info.apiVersion)
    }
}
