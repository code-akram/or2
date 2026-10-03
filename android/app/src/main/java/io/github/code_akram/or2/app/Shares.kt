package io.github.code_akram.or2.app

import android.net.Uri
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.Saver
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.code_akram.or2.connection.ActiveTerminal
import io.github.code_akram.or2.connection.HostConnections
import io.github.code_akram.or2.paste.NO_TERMINAL_FOR_IMAGE
import io.github.code_akram.or2.paste.ShareTarget
import io.github.code_akram.or2.paste.SharePickerSheet
import io.github.code_akram.or2.paste.imageFromUri
import io.github.code_akram.or2.paste.restoredShare
import io.github.code_akram.or2.paste.savedShare
import io.github.code_akram.or2.paste.shareNote

/**
 * Images shared from another app: the open terminal they go to. One question for the whole share (the share picker:
 * the open terminals, the last used first); what it does not send (unreadable items, more than MAX_IMAGES) is said. A
 * pick shows that terminal ([show]) and queues the images there.
 *
 * Saved state: a rotation while the picker is open keeps the images (their read grants belong to the activity). A
 * recreation after the process died drops them (`restoredShare`): the terminals they were for are gone.
 */
@Composable
internal fun ImageShareRoute(actions: AppActions, connections: HostConnections, terminals: List<ActiveTerminal>, show: (Long) -> Unit) {
    val shareOffer by actions.imageShares.request.collectAsStateWithLifecycle()
    var sharing by rememberSaveable(stateSaver = SHARE_SAVER) { mutableStateOf<List<Uri>?>(null) }
    LaunchedEffect(shareOffer) {
        if (shareOffer == null) return@LaunchedEffect
        val share = actions.imageShares.take() ?: return@LaunchedEffect
        when {
            share.images.isEmpty() -> actions.message(shareNote(share))
            connections.shareTargets().isEmpty() -> actions.message(NO_TERMINAL_FOR_IMAGE)
            else -> {
                sharing = share.images
                shareNote(share)?.let(actions.message)
            }
        }
    }
    val appContext = LocalContext.current.applicationContext
    val uris = sharing ?: return
    val targets = remember(terminals) { connections.shareTargets() }
    if (targets.isEmpty()) {
        LaunchedEffect(Unit) {
            sharing = null
            actions.message(NO_TERMINAL_FOR_IMAGE)
        }
        return
    }
    SharePickerSheet(
        targets.map { ShareTarget(it.id, it.host.label, it.title) },
        pick = { target ->
            sharing = null
            connections.terminal(target.id)?.let { terminal ->
                show(terminal.id)
                for (uri in uris) terminal.imagePaste?.start(imageFromUri(appContext, uri))
            }
        },
        dismiss = { sharing = null },
    )
}

/** A pending share's images as saved state, with the process that took them ([savedShare], [restoredShare]). */
private val SHARE_SAVER: Saver<List<Uri>?, Array<String>> = Saver(
    save = { uris -> uris?.let { savedShare(*it.map(Uri::toString).toTypedArray()) } },
    restore = { saved -> restoredShare(saved)?.map(Uri::parse) },
)
