package io.github.code_akram.or2.terminal

import androidx.compose.foundation.layout.Arrangement
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
 * The compact gestures-and-shortcuts sheet (Ctrl+Shift+/, and the Terminals sheet's row): the touch gestures first
 * ([TouchRows], with a muted note on what the swipes move), then the hardware-keyboard shortcuts
 * ([KeyboardShortcutRows]); grouped cards of dense rows, the gesture or keys in small mono text and what they do in
 * muted secondary text.
 */
@Composable
fun ShortcutsSheet(dismiss: () -> Unit) {
    Or2Sheet(dismiss, title = "Gestures & shortcuts", modifier = Modifier.testTag("shortcuts-sheet")) {
        SectionHeader("Touch", topGap = 0.dp)
        ShortcutGroup(TouchRows, Modifier.testTag("shortcuts-touch"))
        Text(
            SWIPES_NOTE, style = Or2Type.Secondary, color = Or2Colors.TextMuted,
            modifier = Modifier.padding(start = Or2Dimens.Gutter, top = 6.dp),
        )
        SectionHeader("Keyboard", topGap = 12.dp)
        ShortcutGroup(KeyboardShortcutRows, Modifier.testTag("shortcuts-keyboard"))
    }
}

@Composable
private fun ShortcutGroup(rows: List<Pair<String, String>>, modifier: Modifier = Modifier) {
    GroupCard(modifier) {
        rows.forEachIndexed { index, (keys, action) ->
            if (index > 0) GroupDivider()
            Row(
                Modifier.fillMaxWidth().padding(horizontal = Or2Dimens.Gutter, vertical = 6.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Text(keys, style = Or2Type.MonoSmall, color = Or2Colors.Text, maxLines = 1, modifier = Modifier.width(ShortcutKeysWidth))
                Text(action, style = Or2Type.Secondary, color = Or2Colors.TextMuted)
            }
        }
    }
}

/** The gesture or keys column: wide enough for `Two fingers ← / →` and `Ctrl+Shift+Enter` in mono 10.5 sp. */
private val ShortcutKeysWidth = 128.dp
