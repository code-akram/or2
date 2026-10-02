package io.github.code_akram.or2.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp

/** How much a notice matters: it tints the strip's icon and text. */
enum class NoticeTone { Info, Warning }

/** The strip's tint: muted for information, `attention` for a warning. */
fun noticeColor(tone: NoticeTone): Color = when (tone) {
    NoticeTone.Info -> Or2Colors.TextMuted
    NoticeTone.Warning -> Or2Colors.Attention
}

/** The icon a strip shows when the caller gives none. */
fun noticeIcon(tone: NoticeTone): ImageVector = when (tone) {
    NoticeTone.Info -> Or2Icons.Info
    NoticeTone.Warning -> Or2Icons.Warning
}

/**
 * One compact status line under a header (connecting, closed, later notices): 28 dp tall, the 12 dp
 * gutter, a 14 dp icon (or, while [busy], a small accent spinner in its place), one line of mono
 * `MonoSmall` text tinted by [tone], and an optional trailing text action in `accent`. It takes layout
 * space like any row, so it never covers what is below it; it has no fill of its own and sits on its
 * container's (the terminal header's tone).
 */
@Composable
fun NoticeStrip(
    text: String, modifier: Modifier = Modifier, tone: NoticeTone = NoticeTone.Info, icon: ImageVector? = null,
    busy: Boolean = false, actionLabel: String? = null, onAction: () -> Unit = {}, actionModifier: Modifier = Modifier,
    textModifier: Modifier = Modifier,
) {
    val color = noticeColor(tone)
    Row(
        modifier.fillMaxWidth().heightIn(min = Or2Dimens.NoticeStrip).padding(start = Or2Dimens.Gutter, end = Or2Dimens.Gutter - 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(Or2Dimens.NoticeIcon), contentAlignment = Alignment.Center) {
            if (busy) Spinner(size = Or2Dimens.NoticeIcon - 2.dp)
            else Icon(icon ?: noticeIcon(tone), null, Modifier.size(Or2Dimens.NoticeIcon), tint = color)
        }
        Spacer(Modifier.width(8.dp))
        Text(
            text, style = Or2Type.MonoSmall, color = color, maxLines = 1, overflow = TextOverflow.Ellipsis,
            modifier = textModifier.weight(1f),
        )
        if (actionLabel != null) {
            // Drawn at the strip's own height; the platform grows the target to 48 dp.
            Box(
                actionModifier.heightIn(min = Or2Dimens.NoticeStrip).clip(Or2Shapes.Pill)
                    .clickable(role = Role.Button, onClick = onAction).padding(horizontal = 6.dp),
                contentAlignment = Alignment.Center,
            ) {
                Text(actionLabel, style = Or2Type.Chip, color = Or2Colors.Accent, maxLines = 1)
            }
        }
    }
}
