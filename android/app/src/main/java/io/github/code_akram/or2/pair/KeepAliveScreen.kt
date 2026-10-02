package io.github.code_akram.or2.pair

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import io.github.code_akram.or2.ui.BottomInsetSpacer
import io.github.code_akram.or2.ui.Or2Colors
import io.github.code_akram.or2.ui.Or2Dimens
import io.github.code_akram.or2.ui.Or2Type
import io.github.code_akram.or2.ui.PrimaryButton
import io.github.code_akram.or2.ui.TextAction
import io.github.code_akram.or2.ui.TopBar
import io.github.code_akram.or2.ui.scrolledUnder

/** The battery step's title and its one line on why. */
const val KEEP_ALIVE_TITLE = "Keep sessions alive in the background?"
const val KEEP_ALIVE_WHY = "Android may stop the connection while or2 is in the background."

/**
 * The last step of adding a host, once ever (`BatteryPrompt`): or2's short explanation before Android's own
 * battery-optimisation dialog. **Allow** opens that dialog; **Not now** (or Back) skips it, and Home keeps a small
 * card to allow it later. While Android's dialog is up ([waiting]) the buttons are off.
 */
@Composable
fun KeepAliveScreen(waiting: Boolean, allow: () -> Unit, notNow: () -> Unit) {
    // Back is "Not now"; while Android's dialog is up there is nothing to answer.
    BackHandler { if (!waiting) notNow() }
    val scroll = rememberScrollState()
    Column(Modifier.fillMaxSize()) {
        TopBar(scrolled = scroll.scrolledUnder())
        Column(
            Modifier.weight(1f).verticalScroll(scroll).padding(horizontal = Or2Dimens.Gutter * 2).testTag("keepalive"),
            horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Spacer(Modifier.height(48.dp))
            Text(
                KEEP_ALIVE_TITLE, style = Or2Type.CardTitle, color = Or2Colors.Text, textAlign = TextAlign.Center,
                modifier = Modifier.semantics { heading() }.testTag("keepalive-title"),
            )
            Text(
                KEEP_ALIVE_WHY, style = Or2Type.Secondary, color = Or2Colors.TextMuted, textAlign = TextAlign.Center,
                modifier = Modifier.testTag("keepalive-why"),
            )
            Spacer(Modifier.height(4.dp))
            PrimaryButton("Allow", allow, Modifier.testTag("keepalive-allow"), enabled = !waiting)
            TextAction("Not now", notNow, Modifier.testTag("keepalive-not-now"), color = Or2Colors.Text, enabled = !waiting)
            Spacer(Modifier.height(24.dp))
            BottomInsetSpacer()
        }
    }
}
