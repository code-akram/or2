package io.github.code_akram.or2.terminal

import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Path
import android.graphics.Rect
import android.graphics.RectF
import kotlin.math.hypot
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt

/*
 * Characters the terminal draws itself, on the cell's pixel grid, instead of through a font: box drawing
 * (U+2500–257F), block elements (U+2580–259F), the media symbols ⏴⏵⏶⏷⏸⏹⏺ (U+23F4–23FA), the elbows ⎾⎿
 * (U+23BE–23BF, Claude Code's tool-output bracket) and Powerline's separators (U+E0B0–E0B7). Neither the
 * bundled fonts nor the phone's have ⏵, ⏺ (but as an emoji) or ⎿ in a monospace face, and font glyphs for
 * blocks do not fill a cell, which leaves seams through block art such as Claude Code's mascot.
 * Ghostty, kitty and Alacritty draw the same ranges themselves.
 */

/** The code point of [text] when it is a single character the terminal draws itself, else -1. */
internal fun spriteOf(text: String): Int {
    if (text.isEmpty() || text.length > 2) return -1
    val codePoint = text.codePointAt(0)
    return if (Character.charCount(codePoint) == text.length && isSprite(codePoint)) codePoint else -1
}

internal fun isSprite(codePoint: Int): Boolean =
    codePoint in 0x2500..0x259F || codePoint in 0x23F4..0x23FA || codePoint in 0x23BE..0x23BF || codePoint in 0xE0B0..0xE0B7

/** A filled rectangle in cell pixels; [alpha] (0–255) scales the cell's own, for the shades ░▒▓. */
internal data class SpriteRect(val left: Int, val top: Int, val right: Int, val bottom: Int, val alpha: Int = 255)

/**
 * What sprites take from the text font: the light line thickness in pixels, and the top and bottom of a
 * capital letter (so ⏵ sits where text does). Computed for each font size.
 */
internal data class SpriteFont(val line: Int, val capTop: Float, val capBottom: Float)

internal fun spriteFont(paint: Paint, baseline: Float): SpriteFont {
    val bounds = Rect()
    paint.getTextBounds("H", 0, 1, bounds)
    return SpriteFont(max(1, paint.underlineThickness.roundToInt()), baseline + bounds.top, baseline + bounds.bottom)
}

/**
 * The rectangles of a box-drawing line or block element in a [width] x [height] cell whose light lines
 * are [line] pixels thick; null for the sprites drawn as paths (arcs, diagonals, media, Powerline).
 * Every edge is a whole pixel and a line leaves the cell at the same place whatever character it
 * belongs to, so neighbouring cells join without a seam.
 */
internal fun spriteRects(codePoint: Int, width: Int, height: Int, line: Int): List<SpriteRect>? = when (codePoint) {
    in 0x2504..0x250B -> dashes(codePoint - 0x2504, intArrayOf(3, 3, 3, 3, 4, 4, 4, 4), width, height, line)
    in 0x254C..0x254F -> dashes(codePoint - 0x254C, intArrayOf(2, 2, 2, 2), width, height, line)
    in 0x256D..0x2573 -> null
    in 0x2500..0x257F -> boxRects(BOX_ARMS.substring((codePoint - 0x2500) * 4, (codePoint - 0x2500) * 4 + 4), width, height, line)
    in 0x2580..0x259F -> blockRects(codePoint, width, height)
    else -> null
}

/**
 * Each box-drawing line as its four arms (up, right, down, left): `.` none, `l` light, `h` heavy,
 * `d` double. Blanks are the dashes, arcs and diagonals, drawn otherwise.
 */
private const val BOX_ARMS =
    ".l.l.h.hl.l.h.h.                                .ll..hl..lh..hh." + // 2500
        "..ll..lh..hl..hhll..lh..hl..hh..l..ll..hh..lh..hlll.lhl.hll.llh." + // 2510
        "hlh.hhl.lhh.hhh.l.lll.lhh.lll.hlh.hlh.lhl.hhh.hh.lll.llh.hll.hlh" + // 2520
        ".lhl.lhh.hhl.hhhll.lll.hlh.llh.hhl.lhl.hhh.lhh.hlllllllhlhlllhlh" + // 2530
        "hlllllhlhlhlhllhhhllllhhlhhlhhlhlhhhhlhhhhhlhhhh                " + // 2540
        ".d.dd.d..dl..ld..dd...ld..dl..ddld..dl..dd..l..dd..ld..dldl.dld." + // 2550
        "ddd.l.ldd.dld.dd.dld.ldl.dddld.ddl.ldd.dldlddldldddd            " + // 2560
        "                ...ll....l....l....hh....h....h..h.ll.h..l.hh.l." // 2570

private const val NONE = 0
private const val LIGHT = 1
private const val HEAVY = 2
private const val DOUBLE = 3

private fun boxRects(arms: String, width: Int, height: Int, line: Int): List<SpriteRect> {
    val weights = arms.map { ".lhd".indexOf(it) }
    val (up, right, down, left) = weights
    fun thickness(weight: Int) = when (weight) {
        LIGHT -> line
        HEAVY -> 2 * line
        else -> 0
    }
    val t = line
    // A double line is two light strokes a light line apart: the upper/left one starts at hy/vx.
    val hy = (height - 3 * t) / 2
    val vx = (width - 3 * t) / 2
    // Where an arm meets single (light or heavy) lines crossing its way, it runs through them.
    val vMax = max(thickness(up), thickness(down))
    val hMax = max(thickness(left), thickness(right))
    val leftEnd = if (vMax > 0) (width - vMax) / 2 + vMax else width / 2
    val rightStart = if (vMax > 0) (width - vMax) / 2 else width / 2
    val upEnd = if (hMax > 0) (height - hMax) / 2 + hMax else height / 2
    val downStart = if (hMax > 0) (height - hMax) / 2 else height / 2
    val verticalDouble = up == DOUBLE || down == DOUBLE
    val horizontalDouble = left == DOUBLE || right == DOUBLE
    val rects = ArrayList<SpriteRect>(8)
    when (left) {
        NONE -> Unit
        DOUBLE -> {
            val upper = when { up == DOUBLE -> vx + t; down == DOUBLE -> vx + 3 * t; else -> leftEnd }
            val lower = when { down == DOUBLE -> vx + t; up == DOUBLE -> vx + 3 * t; else -> leftEnd }
            rects += SpriteRect(0, hy, upper, hy + t)
            rects += SpriteRect(0, hy + 2 * t, lower, hy + 3 * t)
        }
        else -> {
            val end = when {
                !verticalDouble -> leftEnd
                right == NONE && up == DOUBLE && down == DOUBLE -> vx + t
                else -> vx + 3 * t
            }
            val top = (height - thickness(left)) / 2
            rects += SpriteRect(0, top, end, top + thickness(left))
        }
    }
    when (right) {
        NONE -> Unit
        DOUBLE -> {
            val upper = when { up == DOUBLE -> vx + 2 * t; down == DOUBLE -> vx; else -> rightStart }
            val lower = when { down == DOUBLE -> vx + 2 * t; up == DOUBLE -> vx; else -> rightStart }
            rects += SpriteRect(upper, hy, width, hy + t)
            rects += SpriteRect(lower, hy + 2 * t, width, hy + 3 * t)
        }
        else -> {
            val start = when {
                !verticalDouble -> rightStart
                left == NONE && up == DOUBLE && down == DOUBLE -> vx + 2 * t
                else -> vx
            }
            val top = (height - thickness(right)) / 2
            rects += SpriteRect(start, top, width, top + thickness(right))
        }
    }
    when (up) {
        NONE -> Unit
        DOUBLE -> {
            val leftStroke = when { left == DOUBLE -> hy + t; right == DOUBLE -> hy + 3 * t; else -> upEnd }
            val rightStroke = when { right == DOUBLE -> hy + t; left == DOUBLE -> hy + 3 * t; else -> upEnd }
            rects += SpriteRect(vx, 0, vx + t, leftStroke)
            rects += SpriteRect(vx + 2 * t, 0, vx + 3 * t, rightStroke)
        }
        else -> {
            val end = when {
                !horizontalDouble -> upEnd
                down == NONE && left == DOUBLE && right == DOUBLE -> hy + t
                else -> hy + 3 * t
            }
            val x = (width - thickness(up)) / 2
            rects += SpriteRect(x, 0, x + thickness(up), end)
        }
    }
    when (down) {
        NONE -> Unit
        DOUBLE -> {
            val leftStroke = when { left == DOUBLE -> hy + 2 * t; right == DOUBLE -> hy; else -> downStart }
            val rightStroke = when { right == DOUBLE -> hy + 2 * t; left == DOUBLE -> hy; else -> downStart }
            rects += SpriteRect(vx, leftStroke, vx + t, height)
            rects += SpriteRect(vx + 2 * t, rightStroke, vx + 3 * t, height)
        }
        else -> {
            val start = when {
                !horizontalDouble -> downStart
                up == NONE && left == DOUBLE && right == DOUBLE -> hy + 2 * t
                else -> hy
            }
            val x = (width - thickness(down)) / 2
            rects += SpriteRect(x, start, x + thickness(down), height)
        }
    }
    return rects
}

/** ┄┅┆┇┈┉┊┋ and ╌╍╎╏: [index] counts light horizontal, heavy horizontal, light vertical, heavy vertical. */
private fun dashes(index: Int, counts: IntArray, width: Int, height: Int, line: Int): List<SpriteRect> {
    val count = counts[index]
    val thickness = if (index % 2 == 0) line else 2 * line
    val vertical = index % 4 >= 2
    val length = if (vertical) height else width
    val segment = length.toFloat() / count
    // Half a gap at each end keeps the rhythm across neighbouring cells.
    val gap = max(1, (segment * 0.35f).roundToInt())
    return List(count) { i ->
        val start = (i * segment + gap / 2f).roundToInt()
        val end = ((i + 1) * segment - gap / 2f).roundToInt()
        if (vertical) {
            val x = (width - thickness) / 2
            SpriteRect(x, start, x + thickness, end)
        } else {
            val y = (height - thickness) / 2
            SpriteRect(start, y, end, y + thickness)
        }
    }
}

private fun blockRects(codePoint: Int, width: Int, height: Int): List<SpriteRect> {
    // Eighths rounded to whole pixels; a half is split the same way wherever it appears.
    fun part(size: Int, eighths: Int) = (size * eighths + 4) / 8
    val midX = part(width, 4)
    val midY = height - part(height, 4)
    return when (codePoint) {
        0x2580 -> listOf(SpriteRect(0, 0, width, midY))
        in 0x2581..0x2588 -> listOf(SpriteRect(0, height - part(height, codePoint - 0x2580), width, height))
        in 0x2589..0x258F -> listOf(SpriteRect(0, 0, part(width, 0x2590 - codePoint), height))
        0x2590 -> listOf(SpriteRect(midX, 0, width, height))
        0x2591 -> listOf(SpriteRect(0, 0, width, height, 64))
        0x2592 -> listOf(SpriteRect(0, 0, width, height, 128))
        0x2593 -> listOf(SpriteRect(0, 0, width, height, 192))
        0x2594 -> listOf(SpriteRect(0, 0, width, part(height, 1)))
        0x2595 -> listOf(SpriteRect(width - part(width, 1), 0, width, height))
        else -> {
            // Quadrants ▖▗▘▙▚▛▜▝▞▟ as bits: upper left 1, upper right 2, lower left 4, lower right 8.
            val quadrants = intArrayOf(4, 8, 1, 13, 9, 7, 11, 2, 6, 14)[codePoint - 0x2596]
            buildList {
                if (quadrants and 1 != 0) add(SpriteRect(0, 0, midX, midY))
                if (quadrants and 2 != 0) add(SpriteRect(midX, 0, width, midY))
                if (quadrants and 4 != 0) add(SpriteRect(0, midY, midX, height))
                if (quadrants and 8 != 0) add(SpriteRect(midX, midY, width, height))
            }
        }
    }
}

/**
 * Draws sprite [codePoint] into a [width] x [height] cell at the canvas origin with [paint]'s colour and
 * alpha. Rectangles are drawn without anti-aliasing (whole pixels, no seams); curves and slopes with it.
 */
internal fun drawSprite(canvas: Canvas, codePoint: Int, width: Int, height: Int, font: SpriteFont, paint: Paint) {
    val alpha = paint.alpha
    val antiAlias = paint.isAntiAlias
    val style = paint.style
    val strokeWidth = paint.strokeWidth
    val rects = spriteRects(codePoint, width, height, font.line)
    if (rects != null) {
        paint.isAntiAlias = false
        paint.style = Paint.Style.FILL
        for (rect in rects) {
            paint.alpha = alpha * rect.alpha / 255
            canvas.drawRect(rect.left.toFloat(), rect.top.toFloat(), rect.right.toFloat(), rect.bottom.toFloat(), paint)
        }
    } else {
        paint.isAntiAlias = true
        drawSpritePath(canvas, codePoint, width.toFloat(), height.toFloat(), font, paint)
    }
    paint.alpha = alpha
    paint.isAntiAlias = antiAlias
    paint.style = style
    paint.strokeWidth = strokeWidth
}

private fun drawSpritePath(canvas: Canvas, codePoint: Int, w: Float, h: Float, font: SpriteFont, paint: Paint) {
    val t = font.line.toFloat()
    val path = Path()
    when (codePoint) {
        0x23BE, 0x23BF -> {
            // ⎾⎿: a light vertical from a capital's top to the baseline, centred like │ (so ⎿ sits under ⏺), and a
            // light stroke from it to the right edge along the top (⎾) or the baseline (⎿). Whole pixels, no blur.
            paint.isAntiAlias = false
            paint.style = Paint.Style.FILL
            val x = ((w.toInt() - font.line) / 2).toFloat()
            val top = font.capTop.roundToInt().toFloat()
            val bottom = font.capBottom.roundToInt().toFloat()
            canvas.drawRect(x, top, x + t, bottom, paint)
            val y = if (codePoint == 0x23BE) top else bottom - t
            canvas.drawRect(x, y, w, y + t, paint)
        }
        in 0x256D..0x2570 -> {
            // ╭╮╯╰: straight where they leave the cell, so they meet the lines next to them exactly.
            paint.style = Paint.Style.STROKE
            paint.strokeWidth = t
            val cx = ((w.toInt() - font.line) / 2) + t / 2
            val cy = ((h.toInt() - font.line) / 2) + t / 2
            val r = minOf(cx, w - cx, cy, h - cy)
            val toRight = codePoint == 0x256D || codePoint == 0x2570
            val toBottom = codePoint == 0x256D || codePoint == 0x256E
            val ox = if (toRight) cx + r else cx - r
            val oy = if (toBottom) cy + r else cy - r
            path.moveTo(if (toRight) w else 0f, cy)
            path.lineTo(ox, cy)
            val start = if (toBottom) 270f else 90f
            val sweep = if (toRight == toBottom) -90f else 90f
            path.arcTo(RectF(ox - r, oy - r, ox + r, oy + r), start, sweep, false)
            path.lineTo(cx, if (toBottom) h else 0f)
            canvas.drawPath(path, paint)
        }
        in 0x2571..0x2573 -> {
            // ╱╲╳ run a little past the corners, so the clip (not a cap) ends them and they join.
            paint.style = Paint.Style.STROKE
            paint.strokeWidth = t
            val length = hypot(w, h)
            val ex = w / length * t
            val ey = h / length * t
            if (codePoint != 0x2572) canvas.drawLine(w + ex, -ey, -ex, h + ey, paint)
            if (codePoint != 0x2571) canvas.drawLine(-ex, -ey, w + ex, h + ey, paint)
        }
        in 0x23F4..0x23FA -> {
            // ⏴⏵⏶⏷⏸⏹⏺: text-sized, centred on a capital letter, not stretched to the cell.
            paint.style = Paint.Style.FILL
            val capHeight = font.capBottom - font.capTop
            // Nearly a capital letter tall; a triangle at most 0.8 of the cell wide, so ⏵⏵ stay apart.
            val size = min(capHeight * 0.95f, w * 0.92f)
            val cx = w / 2
            val cy = (font.capTop + font.capBottom) / 2
            val half = size / 2
            val depth = size * 0.866f / 2 // An equilateral triangle's height, halved.
            when (codePoint) {
                0x23F4 -> path.triangle(cx + depth, cy - half, cx - depth, cy, cx + depth, cy + half)
                0x23F5 -> path.triangle(cx - depth, cy - half, cx + depth, cy, cx - depth, cy + half)
                0x23F6 -> path.triangle(cx - half, cy + depth, cx, cy - depth, cx + half, cy + depth)
                0x23F7 -> path.triangle(cx - half, cy - depth, cx, cy + depth, cx + half, cy - depth)
                0x23F8 -> {
                    val bar = size * 0.3f
                    canvas.drawRect(cx - size * 0.45f, cy - half, cx - size * 0.45f + bar, cy + half, paint)
                    canvas.drawRect(cx + size * 0.45f - bar, cy - half, cx + size * 0.45f, cy + half, paint)
                }
                0x23F9 -> canvas.drawRect(cx - half * 0.85f, cy - half * 0.85f, cx + half * 0.85f, cy + half * 0.85f, paint)
                else -> canvas.drawCircle(cx, cy, half * 0.85f, paint)
            }
            if (!path.isEmpty) canvas.drawPath(path, paint)
        }
        else -> {
            // Powerline: solid and thin arrows (E0B0–E0B3), solid and thin half circles (E0B4–E0B7).
            val solid = codePoint % 2 == 0
            val pointsRight = codePoint == 0xE0B0 || codePoint == 0xE0B1 || codePoint == 0xE0B4 || codePoint == 0xE0B5
            paint.style = if (solid) Paint.Style.FILL else Paint.Style.STROKE
            paint.strokeWidth = t
            val inset = if (solid) 0f else t / 2
            if (codePoint <= 0xE0B3) {
                val base = if (pointsRight) inset else w - inset
                path.moveTo(base, inset)
                path.lineTo(if (pointsRight) w - inset else inset, h / 2)
                path.lineTo(base, h - inset)
            } else {
                val oval = if (pointsRight) RectF(-w + inset, inset, w - inset, h - inset) else RectF(inset, inset, 2 * w - inset, h - inset)
                path.arcTo(oval, if (pointsRight) 270f else 90f, 180f, true)
            }
            if (solid) path.close()
            canvas.drawPath(path, paint)
        }
    }
}

private fun Path.triangle(x1: Float, y1: Float, x2: Float, y2: Float, x3: Float, y3: Float) {
    moveTo(x1, y1)
    lineTo(x2, y2)
    lineTo(x3, y3)
    close()
}
