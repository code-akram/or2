package io.github.code_akram.or2.terminal

import io.github.code_akram.or2.ffi.CellLink
import io.github.code_akram.or2.ffi.CellStyle
import io.github.code_akram.or2.ffi.CellWidth
import io.github.code_akram.or2.ffi.Underline
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class TerminalLinksTest {
    private val style = CellStyle(1u, 0u, null, Underline.NONE, false, false, false, false, false)

    private fun row(text: String, wrapped: Boolean = false, links: List<CellLink> = emptyList()) = ResolvedRow(
        text.map { ResolvedCell(if (it == ' ') "" else "$it", CellWidth.NARROW, style) }, wrapped, links)

    private fun at(rows: List<ResolvedRow>, column: Int, row: Int = 0) = TerminalLinks.at(rows, CellPosition(column, row))

    @Test
    fun aTapOnATextUrlOpensItAndATapBesideItDoesNot() {
        val rows = listOf(row("see https://example.org/a?b=1 ok "))
        val link = at(rows, 10)!!
        assertEquals("https://example.org/a?b=1", link.uri)
        assertEquals(listOf(LinkSpan(0, 4..28)), link.spans)
        assertEquals(link, at(rows, 4))
        assertEquals(link, at(rows, 28))
        assertNull(at(rows, 3))
        assertNull(at(rows, 29))
        assertNull(at(rows, 31))
        assertEquals("http://Example.org", at(listOf(row("http://Example.org")), 0)!!.uri)
        assertEquals("HTTPS://example.org", at(listOf(row("HTTPS://example.org")), 0)!!.uri)
    }

    @Test
    fun aUrlBrokenAcrossWrappedRowsOpensWhole() {
        val rows = listOf(
            row("x: https://ex", wrapped = true),
            row("ample.org/lon", wrapped = true),
            row("g/path. next "),
            row("https://other.example"),
        )
        val expected = TerminalLink(
            "https://example.org/long/path",
            listOf(LinkSpan(0, 3..12), LinkSpan(1, 0..12), LinkSpan(2, 0..5)),
        )
        // A tap on any of its rows finds the same link.
        assertEquals(expected, at(rows, 5, row = 0))
        assertEquals(expected, at(rows, 0, row = 1))
        assertEquals(expected, at(rows, 3, row = 2))
        assertNull(at(rows, 6, row = 2)) // The sentence's full stop.
        // A row that is not a continuation starts its own line.
        assertEquals("https://other.example", at(rows, 0, row = 3)!!.uri)
        // An unwrapped row break ends a URL.
        val broken = listOf(row("https://ex"), row("ample.org"))
        assertEquals("https://ex", at(broken, 2)!!.uri)
        assertNull(at(broken, 2, row = 1))
    }

    @Test
    fun trailingPunctuationIsNotPartOfTheUrl() {
        for (text in listOf("https://example.org.", "https://example.org,", "https://example.org;",
            "https://example.org:", "https://example.org!", "https://example.org?", "https://example.org...",
            "\"https://example.org\"", "'https://example.org'", "<https://example.org>", "**https://example.org**")) {
            val column = text.indexOf('h')
            assertEquals(text, "https://example.org", at(listOf(row(text)), column)!!.uri)
        }
        // Inside the URL they stay.
        assertEquals("https://example.org/a.b?c=d,e", at(listOf(row("https://example.org/a.b?c=d,e.")), 0)!!.uri)
    }

    @Test
    fun bracketsCloseTheirOwnPairOnly() {
        assertEquals("https://example.org/a", at(listOf(row("(see https://example.org/a)")), 6)!!.uri)
        assertEquals("https://example.org/a_(b)", at(listOf(row("(see https://example.org/a_(b))")), 6)!!.uri)
        assertEquals("https://example.org/a_(b)", at(listOf(row("https://example.org/a_(b).")), 0)!!.uri)
        assertEquals("https://example.org/x", at(listOf(row("[https://example.org/x]")), 1)!!.uri)
        assertEquals("https://example.org/[1]", at(listOf(row("https://example.org/[1]")), 0)!!.uri)
        assertEquals("https://example.org/x", at(listOf(row("{https://example.org/x}")), 1)!!.uri)
        assertEquals("https://example.org/x", at(listOf(row("[link](https://example.org/x)")), 8)!!.uri)
    }

    @Test
    fun onlyHttpAndHttpsAreDetected() {
        for (text in listOf("file:///etc/passwd", "intent://scan/#Intent;scheme=zxing;end", "javascript:alert(1)",
            "ftp://example.org/x", "mailto:a@example.org", "xhttps://example.org", "https://", "https:example.org")) {
            assertNull(text, at(listOf(row(text)), 2))
        }
        assertTrue(TerminalLinks.allowed("https://example.org"))
        assertTrue(TerminalLinks.allowed("HTTP://example.org"))
        assertFalse(TerminalLinks.allowed("https://"))
        assertFalse(TerminalLinks.allowed("file:///tmp/x"))
        assertFalse(TerminalLinks.allowed("intent://x#Intent;end"))
        assertFalse(TerminalLinks.allowed("javascript:alert(1)"))
        assertFalse(TerminalLinks.allowed("https:/example.org"))
    }

    @Test
    fun osc8LinksOpenTheirTargetOverTheirCells() {
        val target = CellLink(4u, 7u, "https://example.org/docs")
        val rows = listOf(row("see docs here", links = listOf(target)))
        val link = at(rows, 5)!!
        assertEquals("https://example.org/docs", link.uri)
        assertEquals(listOf(LinkSpan(0, 4..7)), link.spans)
        assertNull(at(rows, 9))
        // A link to a file on the host (`ls --hyperlink`) or another scheme is not opened.
        val file = listOf(row("notes.txt", links = listOf(CellLink(0u, 8u, "file://workstation.local/home/notes.txt"))))
        assertNull(at(file, 2))
        // Its text is still looked at: a URL it shows opens.
        val shown = listOf(row("https://example.org", links = listOf(CellLink(0u, 18u, "intent://x#Intent;end"))))
        assertEquals("https://example.org", at(shown, 3)!!.uri)
    }

    @Test
    fun wideCharactersMapToTheirCellsAndATapOnATailCounts() {
        val cells = "https://example.org/".map { ResolvedCell("$it", CellWidth.NARROW, style) } + listOf(
            ResolvedCell("界", CellWidth.WIDE, style), ResolvedCell("", CellWidth.SPACER_TAIL, style),
            ResolvedCell("", CellWidth.NARROW, style), ResolvedCell("x", CellWidth.NARROW, style),
        )
        val rows = listOf(ResolvedRow(cells, false))
        val link = at(rows, 21)!! // The wide character's tail.
        assertEquals("https://example.org/界", link.uri)
        assertEquals(listOf(LinkSpan(0, 0..21)), link.spans)
        assertNull(at(rows, 23))
    }

    @Test
    fun aTapOutsideTheGridFindsNothing() {
        val rows = listOf(row("https://example.org"))
        assertNull(at(rows, 0, row = 1))
        assertNull(at(rows, 40))
        assertNull(at(emptyList(), 0))
    }
}
