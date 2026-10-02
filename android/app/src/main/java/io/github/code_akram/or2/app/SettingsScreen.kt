package io.github.code_akram.or2.app

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import io.github.code_akram.or2.ui.BottomInsetSpacer
import io.github.code_akram.or2.ui.GroupCard
import io.github.code_akram.or2.ui.ListRow
import io.github.code_akram.or2.ui.Or2Dimens
import io.github.code_akram.or2.ui.Or2Toggle
import io.github.code_akram.or2.ui.SectionHeader
import io.github.code_akram.or2.ui.TopBar
import io.github.code_akram.or2.ui.scrolledUnder

/** The muted sentence under the `Agent notifications` switch. */
const val AGENT_ALERTS_EXPLANATION =
    "One notification when an agent needs input or finishes, on every host shown in the inbox. None for the pane on screen."

/** The Settings destination over the application's [AppSettings] and the agent notification switch. */
@Composable
fun SettingsRoute(agentAlerts: Boolean, setAgentAlerts: (Boolean) -> Unit, back: () -> Unit) {
    val context = LocalContext.current
    val settings = remember { (context.applicationContext as? Or2Application)?.settings ?: AppSettings(MemoryPrefStore()) }
    val copyFromHost by settings.copyFromHost.collectAsState()
    SettingsScreen(
        agentAlerts = agentAlerts, setAgentAlerts = setAgentAlerts,
        copyFromHost = copyFromHost, setCopyFromHost = settings::setCopyFromHost, back = back,
    )
}

/**
 * Settings, pushed from Home: a `TopBar` titled "Settings" and grouped cards under section headers. Every
 * switch defaults to the zero-configuration choice; the screen is the off-ramp, never a setup step.
 */
@Composable
fun SettingsScreen(
    agentAlerts: Boolean,
    setAgentAlerts: (Boolean) -> Unit,
    copyFromHost: Boolean,
    setCopyFromHost: (Boolean) -> Unit,
    back: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val scroll = rememberScrollState()
    Column(modifier.fillMaxSize()) {
        TopBar(title = "Settings", back = back, scrolled = scroll.scrolledUnder())
        Column(Modifier.weight(1f).verticalScroll(scroll).padding(horizontal = Or2Dimens.Gutter).testTag("settings-list")) {
            SectionHeader("Notifications", topGap = 8.dp)
            GroupCard {
                ListRow(
                    "Agent notifications", subtitle = AGENT_ALERTS_EXPLANATION, onClick = { setAgentAlerts(!agentAlerts) },
                    trailing = { Or2Toggle(agentAlerts, setAgentAlerts, Modifier.testTag("settings-agent-alerts")) },
                )
            }
            SectionHeader("Terminal")
            GroupCard {
                ListRow(
                    "Copy from the host", subtitle = "Programs on a host can copy text to this phone's clipboard (OSC 52).",
                    modifier = Modifier.testTag("settings-copy-from-host"), onClick = { setCopyFromHost(!copyFromHost) },
                    trailing = { Or2Toggle(copyFromHost, setCopyFromHost, Modifier.testTag("settings-copy-from-host-switch")) },
                )
            }
            Spacer(Modifier.height(24.dp))
            BottomInsetSpacer()
        }
    }
}
