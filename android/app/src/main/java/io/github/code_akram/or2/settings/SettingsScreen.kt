package io.github.code_akram.or2.settings

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import io.github.code_akram.or2.ui.BottomInsetSpacer
import io.github.code_akram.or2.ui.GroupCard
import io.github.code_akram.or2.ui.ListRow
import io.github.code_akram.or2.ui.Or2Colors
import io.github.code_akram.or2.ui.Or2Dimens
import io.github.code_akram.or2.ui.Or2Toggle
import io.github.code_akram.or2.ui.Or2Type
import io.github.code_akram.or2.ui.SectionHeader
import io.github.code_akram.or2.ui.TopBar

/** The muted sentence under the `Agent notifications` switch. */
const val AGENT_ALERTS_EXPLANATION =
    "One notification when an agent needs input or finishes, on every host shown in the inbox. None for the pane on screen."

/**
 * App-wide switches, pushed from Home: a `TopBar` titled "Settings" and grouped cards under section headers (the
 * grouped settings list of the UI system). Every switch is on by default; the screen is the off-ramp.
 */
@Composable
fun SettingsScreen(
    agentAlerts: Boolean,
    setAgentAlerts: (Boolean) -> Unit,
    back: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Column(modifier.fillMaxSize()) {
        TopBar(title = "Settings", back = back)
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = Or2Dimens.Gutter).testTag("settings-list")) {
            SectionHeader("Notifications", topGap = 8.dp)
            GroupCard {
                ListRow(
                    "Agent notifications", onClick = { setAgentAlerts(!agentAlerts) },
                    trailing = { Or2Toggle(agentAlerts, setAgentAlerts, Modifier.testTag("settings-agent-alerts")) },
                )
            }
            Text(AGENT_ALERTS_EXPLANATION, style = Or2Type.Secondary, color = Or2Colors.TextMuted, modifier = Modifier.padding(top = 6.dp))
            BottomInsetSpacer()
        }
    }
}
