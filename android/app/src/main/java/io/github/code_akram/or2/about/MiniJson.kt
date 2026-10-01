package io.github.code_akram.or2.about

/**
 * A small strict JSON reader for the licence assets. The app has no JSON library (org.json is a
 * stub on the JVM, where the parsing is unit-tested, and a dependency just for three asset files
 * is not worth another entry to attribute): objects become [Map], arrays [List], numbers [Double],
 * and strings, booleans and null are themselves.
 */
internal object MiniJson {
    class ParseException(message: String) : IllegalArgumentException(message)

    fun parse(text: String): Any? {
        val reader = Reader(text)
        val value = reader.value()
        reader.skipWhitespace()
        if (!reader.atEnd) reader.fail("trailing characters")
        return value
    }

    private class Reader(private val text: String) {
        private var pos = 0
        val atEnd get() = pos >= text.length

        fun fail(message: String): Nothing = throw ParseException("$message at offset $pos")

        fun skipWhitespace() {
            while (pos < text.length && text[pos].let { it == ' ' || it == '\n' || it == '\r' || it == '\t' }) pos++
        }

        fun value(): Any? {
            skipWhitespace()
            if (atEnd) fail("unexpected end")
            return when (val c = text[pos]) {
                '{' -> obj()
                '[' -> array()
                '"' -> string()
                't' -> literal("true", true)
                'f' -> literal("false", false)
                'n' -> literal("null", null)
                else -> if (c == '-' || c in '0'..'9') number() else fail("unexpected '$c'")
            }
        }

        private fun literal(word: String, result: Any?): Any? {
            if (!text.startsWith(word, pos)) fail("expected $word")
            pos += word.length
            return result
        }

        private fun number(): Double {
            val start = pos
            while (pos < text.length && text[pos].let { it in '0'..'9' || it == '-' || it == '+' || it == '.' || it == 'e' || it == 'E' }) pos++
            return text.substring(start, pos).toDoubleOrNull() ?: fail("bad number")
        }

        private fun obj(): Map<String, Any?> {
            pos++ // {
            val result = LinkedHashMap<String, Any?>()
            skipWhitespace()
            if (peek() == '}') { pos++; return result }
            while (true) {
                skipWhitespace()
                if (peek() != '"') fail("expected a key")
                val key = string()
                skipWhitespace()
                if (peek() != ':') fail("expected ':'")
                pos++
                result[key] = value()
                skipWhitespace()
                when (peek()) {
                    ',' -> pos++
                    '}' -> { pos++; return result }
                    else -> fail("expected ',' or '}'")
                }
            }
        }

        private fun array(): List<Any?> {
            pos++ // [
            val result = ArrayList<Any?>()
            skipWhitespace()
            if (peek() == ']') { pos++; return result }
            while (true) {
                result += value()
                skipWhitespace()
                when (peek()) {
                    ',' -> pos++
                    ']' -> { pos++; return result }
                    else -> fail("expected ',' or ']'")
                }
            }
        }

        private fun peek(): Char = if (atEnd) fail("unexpected end") else text[pos]

        private fun string(): String {
            pos++ // opening quote
            val out = StringBuilder()
            while (true) {
                if (atEnd) fail("unterminated string")
                val c = text[pos++]
                when {
                    c == '"' -> return out.toString()
                    c == '\\' -> {
                        if (atEnd) fail("unterminated escape")
                        when (val e = text[pos++]) {
                            '"', '\\', '/' -> out.append(e)
                            'b' -> out.append('\b')
                            'f' -> out.append('\u000C')
                            'n' -> out.append('\n')
                            'r' -> out.append('\r')
                            't' -> out.append('\t')
                            'u' -> {
                                if (pos + 4 > text.length) fail("short unicode escape")
                                out.append(text.substring(pos, pos + 4).toIntOrNull(16)?.toChar() ?: fail("bad unicode escape"))
                                pos += 4
                            }
                            else -> fail("bad escape '\\$e'")
                        }
                    }
                    c < ' ' -> fail("control character in string")
                    else -> out.append(c)
                }
            }
        }
    }
}
