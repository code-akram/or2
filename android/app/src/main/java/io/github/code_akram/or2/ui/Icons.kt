package io.github.code_akram.or2.ui

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.addPathNodes
import androidx.compose.ui.unit.dp

/**
 * 24 dp outline icons (1.75 stroke, round caps), drawn from plain SVG path data so the app needs
 * no icon library. Tint them with `Icon(..., tint = ...)`.
 */
object Or2Icons {
    private fun icon(name: String, vararg paths: String, stroke: Float = 1.75f): ImageVector =
        ImageVector.Builder(name, 24.dp, 24.dp, 24f, 24f).apply {
            paths.forEach {
                addPath(
                    addPathNodes(it), stroke = SolidColor(Color.Black), strokeLineWidth = stroke,
                    strokeLineCap = StrokeCap.Round, strokeLineJoin = StrokeJoin.Round,
                )
            }
        }.build()

    val Back = icon("back", "M19 12H5", "M11 5l-7 7 7 7")
    val Close = icon("close", "M6 6l12 12", "M18 6L6 18")
    val Check = icon("check", "M5 12.5l4.5 4.5L19 7")
    val Plus = icon("plus", "M12 5v14", "M5 12h14")
    val Minus = icon("minus", "M5 12h14")
    val ChevronRight = icon("chevron-right", "M9 5l7 7-7 7")
    val ChevronDown = icon("chevron-down", "M5 9l7 7 7-7")
    val ArrowUp = icon("arrow-up", "M6 15l6-6 6 6")
    val ArrowDown = icon("arrow-down", "M6 9l6 6 6-6")
    val ArrowLeft = icon("arrow-left", "M15 6l-6 6 6 6")
    val ArrowRight = icon("arrow-right", "M9 6l6 6-6 6")
    val Server = icon(
        "server",
        "M5 3.5h14a2 2 0 0 1 2 2v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4a2 2 0 0 1 2-2z",
        "M5 12.5h14a2 2 0 0 1 2 2v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4a2 2 0 0 1 2-2z",
        "M7 7.5h.01", "M7 16.5h.01",
    )
    val Inbox = icon(
        "inbox", "M3 13l3-8h12l3 8v5.5a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V13z", "M3 13h5l1 3h6l1-3h5",
    )
    val Home = icon(
        "home", "M3.5 11.5L12 4l8.5 7.5", "M6 10v8.5a1.5 1.5 0 0 0 1.5 1.5H10v-5h4v5h2.5a1.5 1.5 0 0 0 1.5-1.5V10",
    )
    val Key = icon("key", "M15 4a5 5 0 1 1 0 10 5 5 0 0 1 0-10z", "M11.5 13.5L4 21", "M6.5 18.5l2 2", "M9 16l2 2")
    val Warning = icon("warning", "M12 4l9.5 16.5h-19z", "M12 10v4.5", "M12 17.5h.01")
    val Backspace = icon(
        "backspace", "M9 5h11a1 1 0 0 1 1 1v12a1 1 0 0 1-1 1H9l-6.5-7z", "M12.5 9.5l5 5", "M17.5 9.5l-5 5",
    )
    val Enter = icon("enter", "M20 5v7a2 2 0 0 1-2 2H5", "M9 10l-4 4 4 4")
    /** Clear-line: a slanted eraser block with a band and the ground line it rubs on. */
    val Eraser = icon(
        "eraser", "M14.8 3.5l5.7 5.7L9.2 20.5l-5.7-5.7z", "M7.8 10.6l5.6 5.6", "M13 20.5h8",
    )
    val Paste = icon(
        "paste", "M9 3.5h6v3.5H9z", "M8.5 5H7a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V7a2 2 0 0 0-2-2h-1.5",
    )
    val Copy = icon("copy", "M9 9h10a1 1 0 0 1 1 1v10a1 1 0 0 1-1 1H9a1 1 0 0 1-1-1V10a1 1 0 0 1 1-1z", "M16 9V5a1 1 0 0 0-1-1H5a1 1 0 0 0-1 1v10a1 1 0 0 0 1 1h3")
    val History = icon("history", "M4 12a8 8 0 1 0 2.4-5.7", "M4 4.5V9h4.5", "M12 8v4.5l3 1.8")
    val Keyboard = icon(
        "keyboard",
        "M4 6.5h16a1 1 0 0 1 1 1v9a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1v-9a1 1 0 0 1 1-1z",
        "M7 10.5h.01", "M10.5 10.5h.01", "M14 10.5h.01", "M17 10.5h.01", "M8 14.5h8",
    )
    val Chat = icon("chat", "M20.5 11.5a8 8 0 0 1-11.6 7.1L4 20l1.5-4.5A8 8 0 1 1 20.5 11.5z")
    val Sidebar = icon("sidebar", "M4 5h16a1 1 0 0 1 1 1v12a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1z", "M9 5v14")
    val Dpad = icon(
        "dpad",
        "M12 3.8a2.4 2.4 0 1 1 0 4.8 2.4 2.4 0 0 1 0-4.8z", "M12 15.4a2.4 2.4 0 1 1 0 4.8 2.4 2.4 0 0 1 0-4.8z",
        "M3.8 12a2.4 2.4 0 1 1 4.8 0 2.4 2.4 0 0 1-4.8 0z", "M15.4 12a2.4 2.4 0 1 1 4.8 0 2.4 2.4 0 0 1-4.8 0z",
    )
    val Send = icon("send", "M12 19V5", "M6 11l6-6 6 6")
    val Mic = icon(
        "mic", "M12 3.5a3 3 0 0 1 3 3v5a3 3 0 0 1-6 0v-5a3 3 0 0 1 3-3z", "M6 11.5a6 6 0 0 0 12 0", "M12 17.5v3.5",
    )
    val Trash = icon("trash", "M4.5 7h15", "M9.5 7V4.5h5V7", "M6.5 7l1 13h9l1-13", "M10 11v5", "M14 11v5")
    val Pencil = icon("pencil", "M4 20h4L19 9l-4-4L4 16z", "M13 7l4 4")
    val Power = icon("power", "M12 3.5v8", "M7 6.8a7.5 7.5 0 1 0 10 0")

    /** `⋯`: a host card's menu. */
    val More = icon("more", "M5.5 12h.01", "M12 12h.01", "M18.5 12h.01", stroke = 3f)
    val Refresh = icon("refresh", "M20 12a8 8 0 1 1-2.4-5.7", "M20 4.5V9h-4.5")
    /** A shell prompt, `>_`: the picker's "Shell" pill and the Resume card. */
    val Terminal = icon("terminal", "M5 7.5l4.5 4.5L5 16.5", "M12.5 17h6.5")
    val Layers = icon("layers", "M12 3.5l9 5-9 5-9-5z", "M3 12.5l9 5 9-5", "M3 16.5l9 5 9-5")
    val Grid = icon(
        "grid", "M4.5 4.5h5v5h-5z", "M14.5 4.5h5v5h-5z", "M4.5 14.5h5v5h-5z", "M14.5 14.5h5v5h-5z",
    )
    val Minimize = icon("minimize", "M5 12h14")
    val Share = icon("share", "M12 15V4", "M7.5 8.5L12 4l4.5 4.5", "M5 13v6a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-6")
    val Upload = icon("upload", "M12 16V5", "M7.5 9.5L12 5l4.5 4.5", "M5 15v4a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-4")
    val Image = icon("image", "M5 4.5h14a1.5 1.5 0 0 1 1.5 1.5v12a1.5 1.5 0 0 1-1.5 1.5H5A1.5 1.5 0 0 1 3.5 18V6A1.5 1.5 0 0 1 5 4.5z", "M3.5 16l4.5-4.5 4 4 2.5-2.5 6 6", "M15.5 9h.01")
    val QrCode = icon(
        "qr-code",
        "M4.5 4.5h5v5h-5z", "M14.5 4.5h5v5h-5z", "M4.5 14.5h5v5h-5z",
        "M7 7h.01", "M17 7h.01", "M7 17h.01",
        "M14.5 14.5h.01", "M19.5 14.5h.01", "M17 17h.01", "M14.5 19.5h.01", "M19.5 19.5h.01",
    )
    val Info = icon("info", "M12 3.5a8.5 8.5 0 1 1 0 17 8.5 8.5 0 0 1 0-17z", "M12 11v5", "M12 7.8h.01")
    val External = icon("external", "M14 4h6v6", "M20 4l-9 9", "M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5")
    val Document = icon("document", "M7 3.5h7l4 4V20a1 1 0 0 1-1 1H7a1 1 0 0 1-1-1V4.5a1 1 0 0 1 1-1z", "M14 3.5V8h4", "M9 12.5h6", "M9 16h6")
    val Fingerprint = icon("fingerprint", "M7 18c1-2 1-4 1-6a4 4 0 0 1 8 0c0 3 .5 5 1.5 6.5", "M12 12c0 3 0 5-1 7", "M4.5 9.5A8 8 0 0 1 12 4a8 8 0 0 1 7.5 5.5")

    /** Settings: three sliders. */
    val Settings = icon(
        "settings", "M4 6.5h9", "M17 6.5h3", "M15 4.5v4", "M4 12h3", "M11 12h9", "M9 10v4",
        "M4 17.5h11", "M19 17.5h1", "M17 15.5v4",
    )
}
