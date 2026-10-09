package io.github.code_akram.or2.terminal

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class TerminalSpritesTest {
    /** Cell sizes seen on phones (even and odd), with their light line thickness. */
    private val cells = listOf(Triple(25, 50, 2), Triple(24, 49, 3), Triple(13, 27, 1), Triple(8, 17, 1), Triple(42, 85, 4))

    /** Box-drawing lines that are rectangles: all but the dashes, arcs and diagonals. */
    private val lines = (0x2500..0x257F).filterNot { it in 0x2504..0x250B || it in 0x254C..0x254F || it in 0x256D..0x2573 }

    /** Alpha per pixel, `[y][x]`; rectangles of the same cell overlap freely. */
    private fun raster(codePoint: Int, width: Int, height: Int, line: Int): Array<IntArray> {
        val pixels = Array(height) { IntArray(width) }
        for (rect in spriteRects(codePoint, width, height, line)!!) {
            for (y in rect.top until rect.bottom) for (x in rect.left until rect.right) pixels[y][x] = maxOf(pixels[y][x], rect.alpha)
        }
        return pixels
    }

    private fun column(pixels: Array<IntArray>, x: Int) = pixels.map { it[x] > 0 }
    private fun row(pixels: Array<IntArray>, y: Int) = pixels[y].map { it > 0 }

    private fun components(pixels: Array<IntArray>): Int {
        val seen = Array(pixels.size) { BooleanArray(pixels[0].size) }
        var count = 0
        for (y in pixels.indices) for (x in pixels[0].indices) {
            if (pixels[y][x] == 0 || seen[y][x]) continue
            count++
            val stack = ArrayDeque(listOf(x to y))
            while (stack.isNotEmpty()) {
                val (px, py) = stack.removeLast()
                if (py !in pixels.indices || px !in pixels[0].indices || seen[py][px] || pixels[py][px] == 0) continue
                seen[py][px] = true
                stack += listOf(px + 1 to py, px - 1 to py, px to py + 1, px to py - 1)
            }
        }
        return count
    }

    @Test fun onlySingleSpriteCharactersAreSprites() {
        assertEquals(0x2500, spriteOf("─"))
        assertEquals(0x259B, spriteOf("▛"))
        assertEquals(0x23F5, spriteOf("⏵"))
        assertEquals(0x23FA, spriteOf("⏺"))
        assertEquals(0xE0B0, spriteOf(""))
        assertEquals(-1, spriteOf(""))
        assertEquals(-1, spriteOf("a"))
        assertEquals(-1, spriteOf("─́"))
        assertEquals(-1, spriteOf("⏺️")) // Emoji presentation stays with the emoji font.
        assertEquals(-1, spriteOf("😀"))
        assertEquals(-1, spriteOf("✻"))
        assertEquals(-1, spriteOf("⏳"))
        assertEquals(-1, spriteOf("■"))
    }

    @Test fun pathSpritesHaveNoRectangles() {
        (0x256D..0x2573).forEach { assertNull(spriteRects(it, 25, 50, 2)) }
        (0x23F4..0x23FA).forEach { assertNull(spriteRects(it, 25, 50, 2)) }
        (0xE0B0..0xE0B7).forEach { assertNull(spriteRects(it, 25, 50, 2)) }
    }

    @Test fun everyRectangleSpriteDrawsInsideItsCell() {
        for ((width, height, line) in cells) {
            for (codePoint in (0x2500..0x259F).filterNot { it in 0x256D..0x2573 }) {
                val rects = spriteRects(codePoint, width, height, line)
                assertNotNull("U+%04X".format(codePoint), rects)
                assertTrue("U+%04X draws nothing".format(codePoint), rects!!.isNotEmpty())
                rects.forEach {
                    assertTrue("U+%04X $it outside $width x $height".format(codePoint),
                        it.left >= 0 && it.top >= 0 && it.right <= width && it.bottom <= height && it.left < it.right && it.top < it.bottom)
                }
            }
        }
    }

    @Test fun linesLeaveEveryCellWhereTheirNeighboursEnterIt() {
        for ((width, height, line) in cells) {
            // Any line leaving a cell matches ─━═ (or │┃║) exactly, so neighbours meet without a seam or a step.
            val horizontal = listOf(0x2500, 0x2501, 0x2550).map { column(raster(it, width, height, line), 0) }
            val vertical = listOf(0x2502, 0x2503, 0x2551).map { row(raster(it, width, height, line), 0) }
            val none = List(height) { false }
            val noneAcross = List(width) { false }
            for (codePoint in lines) {
                val pixels = raster(codePoint, width, height, line)
                val name = "%c U+%04X at $width x $height".format(codePoint, codePoint)
                for (edge in listOf(column(pixels, 0), column(pixels, width - 1))) {
                    assertTrue("$name: side edge $edge", edge == none || edge in horizontal)
                }
                for (edge in listOf(row(pixels, 0), row(pixels, height - 1))) {
                    assertTrue("$name: top or bottom edge $edge", edge == noneAcross || edge in vertical)
                }
            }
            // ┌ continues the ─ on its right, ╬ the ║ below it.
            assertEquals(column(raster(0x2500, width, height, line), 0), column(raster(0x250C, width, height, line), width - 1))
            assertEquals(row(raster(0x2551, width, height, line), 0), row(raster(0x256C, width, height, line), height - 1))
        }
    }

    @Test fun singleLinesJoinAndDoubleLinesKeepTheirGaps() {
        val doubles = mapOf(
            '═' to 2, '║' to 2, '╔' to 2, '╗' to 2, '╚' to 2, '╝' to 2, '╠' to 3, '╣' to 3, '╦' to 3, '╩' to 3, '╬' to 4,
            '╒' to 1, '╓' to 1, '╕' to 1, '╖' to 1, '╘' to 1, '╙' to 1, '╛' to 1, '╜' to 1,
            '╞' to 1, '╡' to 1, '╥' to 1, '╨' to 1, '╪' to 1, '╫' to 1, '╟' to 2, '╢' to 2, '╤' to 2, '╧' to 2,
        ).mapKeys { it.key.code }
        for ((width, height, line) in cells) {
            for (codePoint in lines) {
                val expected = doubles[codePoint] ?: 1
                assertEquals("%c U+%04X at $width x $height".format(codePoint, codePoint),
                    expected, components(raster(codePoint, width, height, line)))
            }
        }
    }

    @Test fun heavyLinesAreThickerThanLightOnes() {
        val light = raster(0x2500, 25, 50, 2)
        val heavy = raster(0x2501, 25, 50, 2)
        assertEquals(2, column(light, 0).count { it })
        assertEquals(4, column(heavy, 0).count { it })
        // A light line through a heavy one: ┿ keeps the vertical light and the horizontal heavy.
        val cross = raster(0x253F, 25, 50, 2)
        assertEquals(4, column(cross, 0).count { it })
        assertEquals(2, row(cross, 0).count { it })
    }

    @Test fun blocksPartitionTheCell() {
        for ((width, height, line) in cells) {
            fun pixels(codePoint: Int) = raster(codePoint, width, height, line)
            fun covered(vararg codePoints: Int): Array<IntArray> {
                val sum = Array(height) { IntArray(width) }
                codePoints.forEach { cp -> pixels(cp).forEachIndexed { y, row -> row.forEachIndexed { x, a -> if (a > 0) sum[y][x]++ } } }
                return sum
            }
            fun assertExactlyOnce(vararg codePoints: Int) {
                val sum = covered(*codePoints)
                assertTrue(codePoints.joinToString { "%c".format(it) } + " at $width x $height", sum.all { row -> row.all { it == 1 } })
            }
            assertExactlyOnce(0x2588)
            assertExactlyOnce(0x2580, 0x2584) // ▀▄
            assertExactlyOnce(0x258C, 0x2590) // ▌▐
            assertExactlyOnce(0x2598, 0x259D, 0x2596, 0x2597) // ▘▝▖▗
            assertExactlyOnce(0x259B, 0x2597) // ▛ is all but ▗
            assertExactlyOnce(0x259C, 0x2596) // ▜ is all but ▖
            assertExactlyOnce(0x2599, 0x259D) // ▙ is all but ▝
            assertExactlyOnce(0x259F, 0x2598) // ▟ is all but ▘
            assertExactlyOnce(0x259A, 0x259E) // ▚▞
            for (k in 1..7) {
                assertTrue(covered(0x2580 + k).sumOf { it.sum() } < covered(0x2581 + k).sumOf { it.sum() })
                assertTrue(covered(0x2590 - k).sumOf { it.sum() } < covered(0x258F - k).sumOf { it.sum() })
            }
            assertEquals(listOf(64, 128, 192), (0x2591..0x2593).map { pixels(it)[0][0] })
        }
    }

    @Test fun claudeCodesMascotHasNoSeams() {
        // The top row of the mascot: ▐▛███▜▌ — the upper half is one solid bar from ▐'s middle to ▌'s.
        val (width, height, line) = cells[0]
        val rows = "▐▛███▜▌".map { raster(it.code, width, height, line) }
        val upper = height / 2 - 1
        val bar = rows.flatMap { it[upper].toList() }
        val first = bar.indexOfFirst { it > 0 }
        val last = bar.indexOfLast { it > 0 }
        assertEquals((width * 4 + 4) / 8, first)
        assertEquals(6 * width + (width * 4 + 4) / 8 - 1, last)
        assertTrue("A seam in the mascot's top row", bar.subList(first, last + 1).all { it == 255 })
        // ▛ and ▜ leave their eyes open below the middle.
        assertEquals(0, rows[1][height - 1][width - 1])
        assertEquals(0, rows[5][height - 1][0])
    }
}
