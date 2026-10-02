package io.github.code_akram.or2.keys

import androidx.compose.foundation.layout.size
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.material3.Icon
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import io.github.code_akram.or2.data.KeyRecord
import io.github.code_akram.or2.ui.GroupCard
import io.github.code_akram.or2.ui.GroupDivider
import io.github.code_akram.or2.ui.ListRow
import io.github.code_akram.or2.ui.Or2Colors
import io.github.code_akram.or2.ui.Or2Dimens
import io.github.code_akram.or2.ui.Or2Icons

/** The **New key** row's subtitle, on the pairing review and the host form alike. */
const val NEW_KEY_SUBTITLE = "Ed25519, generated on this phone. Asks for your biometric to save it."

/**
 * The key a host uses, as a radio group: the stored keys with their short fingerprints, then **New key** (made on the
 * phone and saved with the biometric when the screen submits). Rows are tagged `<tagPrefix>:<key id>` and
 * `<tagPrefix>-new`. Used by the Easy pair review and the host form.
 */
@Composable
fun KeyPicker(
    keys: List<KeyRecord>, selectedKeyId: String?, newSelected: Boolean, enabled: Boolean,
    choose: (String) -> Unit, chooseNew: () -> Unit, tagPrefix: String, modifier: Modifier = Modifier,
) {
    GroupCard(modifier.selectableGroup()) {
        keys.forEach { key ->
            val selected = !newSelected && selectedKeyId == key.id
            ListRow(
                key.label, subtitle = shortFingerprint(key.fingerprint), subtitleMono = true, icon = Or2Icons.Key,
                modifier = Modifier.testTag("$tagPrefix:${key.id}").semantics(mergeDescendants = true) {}
                    .selectable(selected, enabled = enabled, role = Role.RadioButton) { choose(key.id) },
                trailing = if (selected) ({ Icon(Or2Icons.Check, null, Modifier.size(Or2Dimens.Icon), tint = Or2Colors.Accent) }) else null,
            )
            GroupDivider(inset = 44.dp)
        }
        ListRow(
            "New key", subtitle = NEW_KEY_SUBTITLE, icon = Or2Icons.Plus,
            modifier = Modifier.testTag("$tagPrefix-new").semantics(mergeDescendants = true) {}
                .selectable(newSelected, enabled = enabled, role = Role.RadioButton, onClick = chooseNew),
            trailing = if (newSelected) ({ Icon(Or2Icons.Check, null, Modifier.size(Or2Dimens.Icon), tint = Or2Colors.Accent) }) else null,
        )
    }
}
