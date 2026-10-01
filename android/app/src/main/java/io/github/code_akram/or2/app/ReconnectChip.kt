package io.github.code_akram.or2.app

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
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
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import io.github.code_akram.or2.connection.ReconnectOffer
import io.github.code_akram.or2.ui.Or2Colors
import io.github.code_akram.or2.ui.Or2Dimens
import io.github.code_akram.or2.ui.Or2Icons
import io.github.code_akram.or2.ui.Or2Shapes
import io.github.code_akram.or2.ui.Or2Type
import io.github.code_akram.or2.ui.StatusDot

/** What the chip says: who is offered, and what reconnecting asks of the user (the grouped unlock). */
fun reconnectChipLabel(offer: ReconnectOffer): String {
    val who = if (offer.hosts.size == 1) offer.hosts[0].label else "${offer.hosts.size} hosts"
    val asks = if (offer.prompts == 1) "1 fingerprint" else "${offer.prompts} fingerprints"
    return "Reconnect $who · $asks"
}

/**
 * The reconnect offer when the app returns: a chip, not a dialog, so a live pane is never covered
 * and nothing waits for an answer. Tapping the label starts the grouped unlock (one biometric per
 * distinct key); the close glyph dismisses it. It is the `surface` status chip of the compact scale
 * with the `attention` dot.
 */
@Composable
fun ReconnectChip(offer: ReconnectOffer, reconnect: () -> Unit, dismiss: () -> Unit, modifier: Modifier = Modifier) {
    Row(
        modifier.clip(Or2Shapes.Pill).background(Or2Colors.Surface).heightIn(min = Or2Dimens.Chip).testTag("reconnect-chip"),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Row(
            Modifier.clickable(role = Role.Button, onClick = reconnect).heightIn(min = Or2Dimens.Chip)
                .padding(start = 12.dp, end = 8.dp, top = 4.dp, bottom = 4.dp).testTag("reconnect-confirm"),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            StatusDot(Or2Colors.Attention)
            Spacer(Modifier.width(8.dp))
            Text(reconnectChipLabel(offer), style = Or2Type.Chip, color = Or2Colors.Text, maxLines = 1)
        }
        Icon(
            Or2Icons.Close, null,
            Modifier.clickable(role = Role.Button, onClick = dismiss).padding(end = 10.dp, top = 6.dp, bottom = 6.dp, start = 2.dp)
                .size(16.dp).semantics { contentDescription = "Dismiss reconnect" }.testTag("reconnect-dismiss"),
            tint = Or2Colors.TextMuted,
        )
    }
}
