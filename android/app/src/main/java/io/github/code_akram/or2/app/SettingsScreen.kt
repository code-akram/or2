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

/** The Settings destination over the application's [AppSettings]. */
@Composable
fun SettingsRoute(back: () -> Unit) {
    val context = LocalContext.current
    val settings = remember { (context.applicationContext as? Or2Application)?.settings ?: AppSettings(MemoryPrefStore()) }
    val copyFromHost by settings.copyFromHost.collectAsState()
    SettingsScreen(copyFromHost = copyFromHost, setCopyFromHost = settings::setCopyFromHost, back = back)
}

/** Settings: a grouped list of switches, each defaulting to the zero-configuration choice. */
@Composable
fun SettingsScreen(copyFromHost: Boolean, setCopyFromHost: (Boolean) -> Unit, back: () -> Unit, modifier: Modifier = Modifier) {
    Column(modifier.fillMaxSize()) {
        TopBar(title = "Settings", back = back)
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = Or2Dimens.Gutter).testTag("settings-list")) {
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
