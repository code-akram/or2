package io.github.code_akram.or2.terminal

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import io.github.code_akram.or2.ui.GroupCard
import io.github.code_akram.or2.ui.GroupDivider
import io.github.code_akram.or2.ui.Or2Colors
import io.github.code_akram.or2.ui.Or2Dimens
import io.github.code_akram.or2.ui.Or2Sheet
import io.github.code_akram.or2.ui.Or2Type
import io.github.code_akram.or2.ui.SectionHeader

/**
 * The compact list of hardware-keyboard shortcuts and terminal gestures (Ctrl+Shift+/): two grouped
 * cards of dense rows, the keys in small mono text and what they do in muted secondary text.
 */
@Composable
fun ShortcutsSheet(dismiss: () -> Unit) {
    Or2Sheet(dismiss, title = "Shortcuts", modifier = Modifier.testTag("shortcuts-sheet")) {
        Column(Modifier.padding(horizontal = Or2Dimens.Gutter).padding(bottom = Or2Dimens.Gutter)) {
            SectionHeader("KEYBOARD", topGap = 0.dp)
            ShortcutGroup(KeyboardShortcutRows)
            SectionHeader("GESTURES", topGap = 12.dp)
            ShortcutGroup(GestureRows)
        }
    }
}

@Composable
private fun ShortcutGroup(rows: List<Pair<String, String>>) {
    GroupCard {
        rows.forEachIndexed { index, (keys, action) ->
            if (index > 0) GroupDivider()
            Row(
                Modifier.fillMaxWidth().padding(horizontal = Or2Dimens.Gutter, vertical = 6.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Text(keys, style = Or2Type.MonoSmall, color = Or2Colors.Text, maxLines = 1, modifier = Modifier.width(128.dp))
                Text(action, style = Or2Type.Secondary, color = Or2Colors.TextMuted)
            }
        }
    }
}
