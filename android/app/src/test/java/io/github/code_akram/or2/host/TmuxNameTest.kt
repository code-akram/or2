package io.github.code_akram.or2.host

import org.junit.Assert.*
import org.junit.Test

class TmuxNameTest {
    @Test
    fun namesFollowTheRustRules() {
        for (name in listOf("work", "my work", "a-b_c", "é界", "x".repeat(128))) assertNull(name, tmuxNameError(name))
        assertEquals("Enter a session name.", tmuxNameError(""))
        assertNotNull(tmuxNameError("x".repeat(129)))
        // The limit is in bytes: 43 three-byte characters are 129 bytes.
        assertNotNull(tmuxNameError("界".repeat(43)))
        assertNull(tmuxNameError("界".repeat(42)))
        for (bad in listOf("a:b", "a.b", "a\\b", "a\nb", "a\u0000b")) assertNotNull(bad, tmuxNameError(bad))
    }
}
