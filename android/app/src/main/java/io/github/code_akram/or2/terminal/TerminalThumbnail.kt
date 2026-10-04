package io.github.code_akram.or2.terminal

import android.graphics.Paint
import android.graphics.Typeface
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.graphics.drawscope.drawIntoCanvas
import androidx.compose.ui.graphics.nativeCanvas
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.code_akram.or2.connection.ActiveTerminal
import io.github.code_akram.or2.connection.HostConnections
import io.github.code_akram.or2.ffi.CellWidth
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalTransport
import io.github.code_akram.or2.ui.Or2Colors
import io.github.code_akram.or2.ui.Or2Type
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.conflate
import kotlin.math.ceil
import kotlin.math.floor
import kotlin.math.max

/** The transport a terminal runs over, shown as a pill (`SSH`, or `Mosh` in the teal one). */
enum class Transport(val label: String) { SSH("SSH"), MOSH("Mosh") }

/** The pill for what the session really runs over ([ActiveTerminal.transport]). */
fun TerminalTransport.display(): Transport = when (this) {
    TerminalTransport.SSH -> Transport.SSH
    TerminalTransport.MOSH -> Transport.MOSH
}

/**
 * A live, scaled-down picture of a terminal for the SESSIONS section on Home. It takes the
 * terminal's frames itself while the terminal screen is not composed (Home and the terminal
 * screen are never on screen together), holding a display lease so the native handle outlives
 * it, and asks for a full frame first so it never starts from a delta.
 */
@Composable
fun TerminalThumbnail(terminal: ActiveTerminal, holder: HostConnections, modifier: Modifier = Modifier) {
    val handle by terminal.handle.collectAsStateWithLifecycle()
    val state by terminal.state.collectAsStateWithLifecycle()
    val grid = remember(terminal) { TerminalGrid() }
    var version by remember(terminal) { mutableIntStateOf(0) }
    DisposableEffect(terminal) {
        holder.attachDisplay(terminal)
        onDispose { holder.detachDisplay(terminal) }
    }
    LaunchedEffect(terminal, handle) {
        val session = TerminalSession()
        session.bind(handle ?: return@LaunchedEffect)
        session.callOwn { requestFullFrame() }
        terminal.frameReady.conflate().collect {
            session.takeFrame()?.let { frame ->
                if (grid.apply(frame)) version++ else if (grid.needsFullFrame) session.callOwn { requestFullFrame() }
            }
            delay(150) // A glance, not a second terminal: a few redraws a second is plenty.
        }
    }
    Box(modifier) {
        TerminalGridPreview(grid, version, Modifier.fillMaxSize())
        if (!grid.hasGrid) {
            Text(
                if (state is SessionState.Closed) "Closed" else "Waiting for output…",
                style = Or2Type.MonoSmall, color = Or2Colors.TextMuted, modifier = Modifier.align(Alignment.Center),
            )
        }
    }
}

/**
 * Draws [grid] scaled to the width of the box, following the cursor row so the latest output is
 * what shows. [version] is only read so a new frame redraws it.
 */
@Composable
fun TerminalGridPreview(grid: TerminalGrid, version: Int, modifier: Modifier = Modifier) {
    val paint = remember {
        Paint(Paint.ANTI_ALIAS_FLAG).apply {
            typeface = Typeface.MONOSPACE
            textSize = 24f
            terminalTypeface(this).also { typeface = it }
        }
    }
    val cellWidth = remember { ceil(paint.measureText("M")) }
    val cellHeight = remember { ceil(paint.fontMetrics.bottom - paint.fontMetrics.top) }
    val baseline = remember { -paint.fontMetrics.top }
    // Reused by every draw: the preview redraws several times a second per terminal.
    val fill = remember { Paint() }
    val run = remember { StringBuilder() }
    Canvas(modifier.clipToBounds()) {
        if (version < 0 || !grid.hasGrid || grid.columns <= 0) return@Canvas // Reading version subscribes to frames.
        val scale = size.width / (grid.columns * cellWidth)
        val visibleRows = max(1, floor(size.height / (cellHeight * scale)).toInt())
        val cursorRow = grid.cursor?.row?.toInt() ?: grid.rows.lastIndex
        val first = (cursorRow + 1 - visibleRows).coerceIn(0, max(0, grid.rows.size - visibleRows))
        drawIntoCanvas { composeCanvas ->
            val canvas = composeCanvas.nativeCanvas
            canvas.save()
            canvas.scale(scale, scale)
            for (index in first until minOf(grid.rows.size, first + visibleRows + 1)) {
                val row = grid.rows[index]
                val y = (index - first) * cellHeight
                // Backgrounds that differ from the screen's, merged into runs.
                var column = 0
                while (column < row.cells.size) {
                    val background = row.cells[column].style.background
                    var end = column + 1
                    while (end < row.cells.size && row.cells[end].style.background == background) end++
                    if (background != grid.background) {
                        fill.color = background.toInt() or (0xff shl 24)
                        canvas.drawRect(column * cellWidth, y, end * cellWidth, y + cellHeight, fill)
                    }
                    column = end
                }
                // Text, one run per stretch of narrow cells in the same colour.
                column = 0
                while (column < row.cells.size) {
                    val cell = row.cells[column]
                    if (cell.width == CellWidth.SPACER_TAIL) { column++; continue }
                    paint.color = cell.style.foreground.toInt() or (0xff shl 24)
                    paint.alpha = if (cell.style.faint) 128 else 255
                    if (cell.width == CellWidth.WIDE) {
                        if (cell.text.isNotBlank()) canvas.drawText(cell.text, column * cellWidth, y + baseline, paint)
                        column += 2
                        continue
                    }
                    var end = column
                    run.setLength(0)
                    while (end < row.cells.size) {
                        val next = row.cells[end]
                        if (next.width != CellWidth.NARROW || next.style.foreground != cell.style.foreground) break
                        run.append(next.text.ifEmpty { " " })
                        end++
                    }
                    if (run.isNotBlank()) canvas.drawText(run.toString(), column * cellWidth, y + baseline, paint)
                    column = max(end, column + 1)
                }
            }
            grid.cursor?.let { cursor ->
                if (cursor.row.toInt() in first..(first + visibleRows)) {
                    fill.color = cursor.color.toInt() or (0xff shl 24)
                    fill.alpha = 160
                    val x = cursor.column.toInt() * cellWidth
                    val y = (cursor.row.toInt() - first) * cellHeight
                    canvas.drawRect(x, y, x + cellWidth, y + cellHeight, fill)
                }
            }
            canvas.restore()
        }
    }
}
