package io.github.code_akram.or2.session

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.material3.ripple
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.layout.layoutId
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.terminal.Transport
import io.github.code_akram.or2.ui.NoticeTone
import io.github.code_akram.or2.ui.Or2Colors
import io.github.code_akram.or2.ui.Or2Dimens
import io.github.code_akram.or2.ui.Or2Icons
import io.github.code_akram.or2.ui.Or2Shapes
import io.github.code_akram.or2.ui.Or2Type
import kotlin.math.max

/** Between the host and the target in the header's title. */
const val TITLE_SEPARATOR = " · "

/** The header's title as plain text: `workstation · tmux main`. */
fun headerTitleText(host: String, target: String) = host + TITLE_SEPARATOR + target

/**
 * The widest the centred title may be: the row's [width] less, on *both* sides, the wider of the
 * [leading] and [trailing] elements and a [gap]. Symmetric, so the title stays centred on the whole
 * width however unequal the sides are, and ellipsizes before it reaches either.
 */
fun centredSlotWidth(width: Int, leading: Int, trailing: Int, gap: Int): Int =
    (width - 2 * (max(leading, trailing) + gap)).coerceAtLeast(0)

/** Where an item of [item] width starts when centred on [width]. */
fun centredX(width: Int, item: Int): Int = (width - item) / 2

/** A one-line status under the terminal header, or null while the session is connected. */
data class TerminalNotice(val text: String, val tone: NoticeTone, val busy: Boolean, val closable: Boolean)

/** Connecting and authenticating are muted with a spinner; a closed session is a warning with its Close action. */
fun terminalNotice(state: SessionState): TerminalNotice? = when (state) {
    SessionState.Connected -> null
    is SessionState.Closed -> TerminalNotice(sessionMessage(state), NoticeTone.Warning, busy = false, closable = true)
    else -> TerminalNotice(sessionMessage(state), NoticeTone.Info, busy = true, closable = false)
}

/**
 * The terminal card's header, 36 dp, one composed piece: a thin drag handle centred at the top, the
 * two round discs as a pair at the left (minimise in `attention`, the sessions sheet in `done`), the
 * title centred on the card's full width (the host in `text`, medium, then the target muted in mono),
 * and at the right the stale-link label ([stale], in `attention`) and the transport pill. The caller
 * gives it its fill and its drag-to-minimise gesture.
 */
@Composable
fun TerminalHeader(
    host: String, target: String, transport: Transport, stale: String?, minimise: () -> Unit, openSwitcher: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Box(modifier.fillMaxWidth().height(Or2Dimens.HeaderRow)) {
        CentredRow(
            Modifier.fillMaxWidth().height(Or2Dimens.HeaderRow),
            leading = {
                // The first disc's edge sits on the 12 dp gutter; each box reaches halfway to the next disc.
                Row(Modifier.padding(start = Or2Dimens.Gutter - Or2Dimens.HeaderDiscGap / 2), verticalAlignment = Alignment.CenterVertically) {
                    HeaderDisc(Or2Icons.Minimize, "Minimise to home", Or2Colors.Attention, minimise, Modifier.testTag("terminal-back"))
                    HeaderDisc(Or2Icons.Sidebar, "Panes and sessions", Or2Colors.Done, openSwitcher, Modifier.testTag("terminal-panes"))
                }
            },
            title = {
                Text(
                    headerTitle(host, target), style = Or2Type.HeaderTitle, maxLines = 1, overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.testTag("terminal-title"),
                )
            },
            trailing = {
                Row(Modifier.padding(end = Or2Dimens.Gutter), verticalAlignment = Alignment.CenterVertically) {
                    if (stale != null) {
                        Text(stale, style = Or2Type.MonoSmall, color = Or2Colors.Attention, maxLines = 1,
                            modifier = Modifier.padding(end = 6.dp).testTag("terminal-link"))
                    }
                    TransportBadge(transport, Modifier.testTag("terminal-transport"), small = true, stale = stale != null)
                }
            },
        )
        // The handle sits above the title, inside the header's own 36 dp: it costs no height.
        Box(
            Modifier.align(Alignment.TopCenter).padding(top = Or2Dimens.TerminalHandleTop)
                .size(Or2Dimens.TerminalHandleWidth, Or2Dimens.TerminalHandleHeight).clip(Or2Shapes.Pill)
                .background(Or2Colors.Handle).testTag("terminal-handle"),
        )
    }
}

/** `host · target`: the host in `text` at the header weight, the separator `subtle`, the target muted mono. */
private fun headerTitle(host: String, target: String): AnnotatedString = buildAnnotatedString {
    withStyle(SpanStyle(color = Or2Colors.Text)) { append(host) }
    withStyle(SpanStyle(color = Or2Colors.Subtle)) { append(TITLE_SEPARATOR) }
    withStyle(Or2Type.HeaderTarget.toSpanStyle().copy(color = Or2Colors.TextMuted)) { append(target) }
}

/**
 * A row of three: [leading] at the start, [trailing] at the end, and [title] centred on the row's whole
 * width in a slot symmetric about the centre ([centredSlotWidth]), so it never meets either side.
 */
@Composable
private fun CentredRow(
    modifier: Modifier, leading: @Composable () -> Unit, title: @Composable () -> Unit, trailing: @Composable () -> Unit,
    gap: Dp = Or2Dimens.HeaderTitleGap,
) {
    Layout(
        content = {
            Box(Modifier.layoutId(LEADING)) { leading() }
            Box(Modifier.layoutId(TITLE)) { title() }
            Box(Modifier.layoutId(TRAILING)) { trailing() }
        },
        modifier = modifier,
    ) { measurables, constraints ->
        val width = constraints.maxWidth
        val loose = constraints.copy(minWidth = 0, minHeight = 0)
        val start = measurables.first { it.layoutId == LEADING }.measure(loose)
        val end = measurables.first { it.layoutId == TRAILING }.measure(loose.copy(maxWidth = (width - start.width).coerceAtLeast(0)))
        val slot = centredSlotWidth(width, start.width, end.width, gap.roundToPx())
        val middle = measurables.first { it.layoutId == TITLE }.measure(loose.copy(maxWidth = slot))
        val height = maxOf(constraints.minHeight, start.height, middle.height, end.height)
        layout(width, height) {
            start.place(0, (height - start.height) / 2)
            middle.place(centredX(width, middle.width), (height - middle.height) / 2)
            end.place(width - end.width, (height - end.height) / 2)
        }
    }
}

private const val LEADING = "leading"
private const val TITLE = "title"
private const val TRAILING = "trailing"

/**
 * A round disc ([Or2Dimens.HeaderButtonDisc], glyph in `background`) centred in a box that reaches halfway to
 * the neighbouring disc; the platform grows the target to 48 dp, and where two grown targets would meet, the
 * box a tap lands in wins, so a tap between the discs goes to the nearer one.
 */
@Composable
private fun HeaderDisc(icon: ImageVector, description: String, color: Color, onClick: () -> Unit, modifier: Modifier = Modifier) {
    Box(
        modifier.size(Or2Dimens.HeaderButtonBox, Or2Dimens.HeaderButtonTouch)
            .clickable(interactionSource = null, indication = ripple(bounded = false, radius = Or2Dimens.HeaderButtonDisc), role = Role.Button, onClick = onClick)
            .semantics { contentDescription = description },
        contentAlignment = Alignment.Center,
    ) {
        Box(Modifier.size(Or2Dimens.HeaderButtonDisc).clip(Or2Shapes.Circle).background(color), contentAlignment = Alignment.Center) {
            Icon(icon, null, Modifier.size(Or2Dimens.HeaderButtonGlyph), tint = Or2Colors.Background)
        }
    }
}
