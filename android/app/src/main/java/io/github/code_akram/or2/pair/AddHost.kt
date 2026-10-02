package io.github.code_akram.or2.pair

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import io.github.code_akram.or2.ui.ActionCard
import io.github.code_akram.or2.ui.Or2Dimens
import io.github.code_akram.or2.ui.Or2Icons
import io.github.code_akram.or2.ui.Or2Sheet

/** Where an add-host card leads. */
enum class AddHostRoute { EASY_PAIR, MANUAL }

/** One card of the add-host chooser: its kicker, title, one-line body, mono meta line and test-tag suffix. */
data class AddHostOption(val route: AddHostRoute, val kicker: String, val title: String, val body: String, val meta: String, val tag: String)

/**
 * The one way to start adding a host, in this order everywhere (Home's empty state, the inbox's and the "+" sheet),
 * like Moshi's: Easy pair is the recommended card (a command on the host and a scan; the phone makes the key), the
 * manual form the second (it can make a key too).
 */
val AddHostOptions = listOf(
    AddHostOption(
        AddHostRoute.EASY_PAIR, "Fastest", "Easy pair with QR",
        "Run one command on your Mac or Linux box and scan the QR. or2 installs the SSH key for you.",
        "Recommended · ~1 min", "easy",
    ),
    AddHostOption(
        AddHostRoute.MANUAL, "SSH-fluent", "Set up manually",
        "Already comfortable with SSH? Enter the hostname, user and key yourself.",
        "~3 min · needs hostname + key", "manual",
    ),
)

/**
 * The add-host cards ([AddHostOptions]). Each card is tagged `<tagPrefix>-<option tag>` (`add-host-easy`,
 * `home-add-host-manual`), and the column `<tagPrefix>-chooser`, so two choosers on screen at once stay apart.
 */
@Composable
fun AddHostChooser(easyPair: () -> Unit, manual: () -> Unit, modifier: Modifier = Modifier, tagPrefix: String = "add-host") {
    Column(modifier.testTag("$tagPrefix-chooser"), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        AddHostOptions.forEach { option ->
            ActionCard(
                option.kicker, option.title, option.body, meta = option.meta,
                icon = when (option.route) {
                    AddHostRoute.EASY_PAIR -> Or2Icons.QrCode
                    AddHostRoute.MANUAL -> Or2Icons.Server
                },
                onClick = when (option.route) {
                    AddHostRoute.EASY_PAIR -> easyPair
                    AddHostRoute.MANUAL -> manual
                },
                modifier = Modifier.testTag("$tagPrefix-${option.tag}"),
            )
        }
    }
}

/** Home's "+": the add-host chooser in a sheet with a handle and no title. */
@Composable
fun AddHostSheet(easyPair: () -> Unit, manual: () -> Unit, dismiss: () -> Unit) {
    Or2Sheet(dismiss, title = null, done = null, modifier = Modifier.testTag("add-host-sheet")) {
        AddHostChooser(easyPair, manual, Modifier.padding(horizontal = Or2Dimens.Gutter).padding(bottom = Or2Dimens.Gutter))
    }
}
