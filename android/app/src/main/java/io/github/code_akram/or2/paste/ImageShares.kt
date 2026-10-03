package io.github.code_akram.or2.paste

import android.content.Intent
import android.net.Uri
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import io.github.code_akram.or2.ui.GroupCard
import io.github.code_akram.or2.ui.GroupDivider
import io.github.code_akram.or2.ui.ListRow
import io.github.code_akram.or2.ui.Or2Icons
import io.github.code_akram.or2.ui.Or2Sheet
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * Images shared to or2 from another app (`ACTION_SEND` or `ACTION_SEND_MULTIPLE` of an image type,
 * contracts.md, "Image paste", "Several images at once"): the activity offers each new share; the UI takes
 * it and asks once which open terminal all its images go to.
 */
class ImageShares {
    private val mutableRequest = MutableStateFlow<Share<Uri>?>(null)
    val request: StateFlow<Share<Uri>?> = mutableRequest.asStateFlow()

    fun offer(share: Share<Uri>) {
        mutableRequest.value = share
    }

    /** Takes the pending share (null when there is none). */
    fun take(): Share<Uri>? = mutableRequest.value.also { mutableRequest.value = null }
}

/**
 * A share's [images], in the order shared, at most [MAX_IMAGES]; how many of its items were [refused] (not a
 * `content:` image, [readableImageScheme]); and how many readable ones were [over] the cap. None is dropped
 * without a word ([shareNote]).
 */
data class Share<T>(val images: List<T>, val refused: Int = 0, val over: Int = 0)

/**
 * The share of [items] (a stream each, null for a missing one), each checked as a single share is: only a
 * `content:` stream ([scheme]) is taken, anything else is refused and counted. The first [MAX_IMAGES] taken
 * are kept, the rest counted as [Share.over].
 */
fun <T : Any> shareOf(items: List<T?>, scheme: (T) -> String?): Share<T> {
    val readable = items.filterNotNull().filter { readableImageScheme(scheme(it)) }
    return Share(readable.take(MAX_IMAGES), refused = items.size - readable.size, over = (readable.size - MAX_IMAGES).coerceAtLeast(0))
}

/**
 * The images an `ACTION_SEND` (one stream) or `ACTION_SEND_MULTIPLE` (a list) of an image type carries, else
 * null. Only `content:` streams are taken ([shareOf], [readableImageScheme]): another app must not have or2
 * open a `file:` path with or2's own rights. A share with nothing to take still comes back, so it is reported.
 */
fun sharedImages(intent: Intent?): Share<Uri>? {
    if (intent == null || intent.type?.startsWith("image/") != true) return null
    val items: List<Uri?> = when (intent.action) {
        Intent.ACTION_SEND -> listOf(intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java))
        Intent.ACTION_SEND_MULTIPLE -> intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java).orEmpty()
        else -> return null
    }
    return shareOf(items) { it.scheme }.takeIf { it.images.isNotEmpty() || it.refused > 0 }
}

/**
 * What the UI says about the items of a share it does not send, or null when it sends them all: the readable
 * images past [MAX_IMAGES], then the items it cannot read.
 */
fun shareNote(share: Share<*>): String? {
    val over = if (share.over > 0) "$TOO_MANY_IMAGES: sending the first $MAX_IMAGES" else null
    val refused = when (share.refused) {
        0 -> null
        1 -> "1 shared item is not an image or2 can read"
        else -> "${share.refused} shared items are not images or2 can read"
    }
    return listOfNotNull(over, refused).joinToString("; ").ifEmpty { null }
}

/** This process, as a pending share's saved state names it ([savedShare]). */
val THIS_PROCESS: String = java.util.UUID.randomUUID().toString()

/** A pending share ([uris]) as saved state: the [process] that took it, then the URIs in order. */
fun savedShare(vararg uris: String, process: String = THIS_PROCESS): Array<String> = arrayOf(process, *uris)

/**
 * A pending share's URIs from saved state, only in the [process] that saved it (a rotation). After the
 * process died the terminals it was for are gone and its read grants need not hold: it is dropped.
 */
fun restoredShare(saved: Array<String>, process: String = THIS_PROCESS): List<String>? =
    saved.takeIf { it.size >= 2 && it[0] == process }?.drop(1)

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
