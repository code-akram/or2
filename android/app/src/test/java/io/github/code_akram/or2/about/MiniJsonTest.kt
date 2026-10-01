package io.github.code_akram.or2.about

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class MiniJsonTest {
    @Test
    fun parsesObjectsArraysAndScalars() {
        val value = MiniJson.parse("""{"a": [1, 2.5, -3e2], "b": {"c": true, "d": false, "e": null}, "f": "x"}""") as Map<*, *>
        assertEquals(listOf(1.0, 2.5, -300.0), value["a"])
        assertEquals(mapOf("c" to true, "d" to false, "e" to null), value["b"])
        assertEquals("x", value["f"])
        assertEquals(emptyList<Any>(), MiniJson.parse(" [ ] "))
        assertEquals(emptyMap<String, Any>(), MiniJson.parse("{}"))
    }

    @Test
    fun decodesEveryEscapeAndKeepsRawUnicode() {
        assertEquals("a\"b\\c/d\b\u000C\n\r\té", MiniJson.parse(""""a\"b\\c\/d\b\f\n\r\té"""") as String)
        assertEquals("© Unicode — 😀", MiniJson.parse("\"© Unicode — 😀\""))
        assertNull(MiniJson.parse("null"))
    }

    @Test
    fun rejectsMalformedInput() {
        for (bad in listOf("", "{", "[1,", """{"a" 1}""", """{"a":1,}""", "[1 2]", "\"abc", """"\q"""", """"\u12"""", "tru", "{} x", "\"a\nb\"")) {
            assertThrows("should reject <$bad>", MiniJson.ParseException::class.java) { MiniJson.parse(bad) }
        }
    }

    @Test
    fun handlesDeepNestingAndLongStrings() {
        val long = "x".repeat(200_000)
        assertEquals(long, MiniJson.parse("\"$long\""))
        val deep = "[".repeat(200) + "]".repeat(200)
        assertTrue(MiniJson.parse(deep) is List<*>)
    }
}
