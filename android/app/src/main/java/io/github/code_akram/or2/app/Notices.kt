package io.github.code_akram.or2.app

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.code_akram.or2.connection.ReconnectOffer
import io.github.code_akram.or2.ui.IconAction
import io.github.code_akram.or2.ui.Or2Card
import io.github.code_akram.or2.ui.Or2Colors
import io.github.code_akram.or2.ui.Or2Dimens
import io.github.code_akram.or2.ui.Or2Icons
import io.github.code_akram.or2.ui.Or2Type
import io.github.code_akram.or2.ui.Spinner

/**
 * The one notices overlay: messages and waiting state as floating cards above the content, never modal, in one order
 * on every screen (the message, a focus or open in progress, the reconnect chip, an unlock in progress). Where it
 * floats is the caller's ([modifier]): at the top of a full-screen terminal, which has no notice area, else above the
 * bottom. Live regions: assistive services announce a new one.
 */
@Composable
internal fun Notices(
    message: String?, focusing: String?, chip: ReconnectOffer?, busy: Boolean,
    dismissMessage: () -> Unit, reconnect: (ReconnectOffer) -> Unit, dismissChip: () -> Unit, modifier: Modifier = Modifier,
) {
    Column(modifier.fillMaxWidth().padding(horizontal = Or2Dimens.Gutter), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        if (message != null) MessageCard(message, dismissMessage)
        if (focusing != null) FocusNotice(focusing)
        chip?.let { offer -> ReconnectChip(offer, reconnect = { reconnect(offer) }, dismiss = dismissChip) }
        if (busy) {
            Or2Card(Modifier.testTag("busy-banner").semantics { liveRegion = LiveRegionMode.Polite }, color = Or2Colors.SurfaceRaised) {
                Row(Modifier.padding(Or2Dimens.Gutter), verticalAlignment = Alignment.CenterVertically) {
                    Spinner()
                    Text("Waiting for authentication or operation…", style = Or2Type.Secondary, color = Or2Colors.TextMuted,
                        modifier = Modifier.padding(start = 12.dp))
                }
            }
        }
    }
}

/** A message the user can dismiss. */
@Composable
private fun MessageCard(message: String, dismiss: () -> Unit) {
    Or2Card(Modifier.testTag("message-banner").semantics { liveRegion = LiveRegionMode.Polite }, color = Or2Colors.SurfaceRaised) {
        Row(Modifier.padding(start = Or2Dimens.Gutter), verticalAlignment = Alignment.CenterVertically) {
            Text(message, style = Or2Type.Secondary, color = Or2Colors.Text, modifier = Modifier.weight(1f).padding(vertical = 8.dp))
            IconAction(Or2Icons.Close, "Dismiss message", dismiss, Modifier.testTag("message-dismiss"), tint = Or2Colors.TextMuted)
        }
    }
}

/** In-place progress while a terminal is being opened or an agent's pane focused: a card with a spinner, never a dialog. */
@Composable
private fun FocusNotice(focusing: String) {
    Or2Card(Modifier.fillMaxWidth().testTag("focus-banner").semantics { liveRegion = LiveRegionMode.Polite }, color = Or2Colors.SurfaceRaised) {
        Row(Modifier.padding(Or2Dimens.Gutter), verticalAlignment = Alignment.CenterVertically) {
            Spinner()
            Text(focusing + "…", style = Or2Type.MonoSmall, color = Or2Colors.TextMuted, maxLines = 1,
                overflow = TextOverflow.Ellipsis, modifier = Modifier.padding(start = 12.dp))
        }
    }
}
