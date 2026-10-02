package io.github.code_akram.or2.paste

import android.content.Intent
import android.net.Uri
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import io.github.code_akram.or2.ui.GroupCard
import io.github.code_akram.or2.ui.GroupDivider
import io.github.code_akram.or2.ui.ListRow
import io.github.code_akram.or2.ui.Or2Dimens
import io.github.code_akram.or2.ui.Or2Icons
import io.github.code_akram.or2.ui.Or2Sheet
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * Images shared to or2 from another app (`ACTION_SEND` of an image type, contracts.md, "Image paste"):
 * the activity offers each new one; the UI takes it and asks which open terminal it goes to.
 */
class ImageShares {
    private val mutableRequest = MutableStateFlow<Uri?>(null)
    val request: StateFlow<Uri?> = mutableRequest.asStateFlow()

    fun offer(uri: Uri) {
        mutableRequest.value = uri
    }

    /** Takes the pending image (null when there is none). */
    fun take(): Uri? = mutableRequest.value.also { mutableRequest.value = null }
}

/** The image an `ACTION_SEND` of an image type carries, else null. */
fun sharedImage(intent: Intent?): Uri? {
    if (intent?.action != Intent.ACTION_SEND || intent.type?.startsWith("image/") != true) return null
    return intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
}

/** What a shared image says when no terminal is open to take it. */
const val NO_TERMINAL_FOR_IMAGE = "No open terminal to send the image to"

/** One open terminal in the share picker: its host and what it shows. */
data class ShareTarget(val id: Long, val host: String, val title: String)

/**
 * The share picker: a compact sheet of the open terminals, the last used first, each one tap that
 * opens it and sends the image there.
 */
@Composable
fun SharePickerSheet(targets: List<ShareTarget>, pick: (ShareTarget) -> Unit, dismiss: () -> Unit) {
    Or2Sheet(dismiss, title = "Send image to", done = null) {
        Column(
            Modifier.padding(horizontal = Or2Dimens.Gutter).padding(bottom = Or2Dimens.Gutter),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            GroupCard(Modifier.testTag("share-picker")) {
                targets.forEachIndexed { index, target ->
                    if (index > 0) GroupDivider()
                    ListRow(
                        target.host, subtitle = target.title, subtitleMono = true, icon = Or2Icons.Terminal,
                        modifier = Modifier.testTag("share-target:${target.id}"), onClick = { pick(target) },
                    )
                }
            }
        }
    }
}
