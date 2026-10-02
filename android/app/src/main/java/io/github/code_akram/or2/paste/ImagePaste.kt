package io.github.code_akram.or2.paste

import io.github.code_akram.or2.ffi.HostException
import io.github.code_akram.or2.session.TerminalNotice
import io.github.code_akram.or2.ui.NoticeTone
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.DelicateCoroutinesApi
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.launch

/** One terminal's image upload, as its notice strip shows it. */
sealed interface UploadState {
    data object Idle : UploadState
    data object Uploading : UploadState

    /** Uploading, and another image just came and was not taken: the strip says so for [ALREADY_SHOWN]. */
    data object AlreadyUploading : UploadState
    data class Failed(val reason: String) : UploadState
}

/** Whether an upload runs (Cancel stops it). */
val UploadState.uploading: Boolean get() = this == UploadState.Uploading || this == UploadState.AlreadyUploading

/** How long the strip says a second image was not taken. */
const val ALREADY_SHOWN = 3_000L

const val ALREADY_UPLOADING = "An image is already uploading"

/** What a host's answer that is no path to type into a terminal fails with ([insertablePath]). */
const val UNUSABLE_PATH = "Upload failed: the host answered an unusable path"

/**
 * A terminal's image paste (contracts.md, "Image paste"): one upload at a time, from any source (the
 * composer's attach button, a keyboard's image, a share), each ending the same way. [start] prepares the
 * image ([prepareImage], off the main thread) and uploads it over the host's connection ([upload]:
 * `upload_image`); the path it returns arrives on [paths] for the terminal screen to insert
 * ([insertTarget], [pathInsertion]), once, even when the screen was not showing at that moment. [cancel]
 * stops it (the upload's coroutine is cancelled, and Rust removes the temporary file). It lives with
 * the terminal in the holder's [scope], not with a view.
 */
class ImagePaste(
    private val scope: CoroutineScope,
    private val upload: suspend (bytes: ByteArray, extension: String) -> String,
) {
    private val mutableState = MutableStateFlow<UploadState>(UploadState.Idle)
    val state: StateFlow<UploadState> = mutableState.asStateFlow()
    private val inserts = Channel<String>(Channel.UNLIMITED)

    /** The uploaded images' paths, each delivered once. */
    val paths: Flow<String> = inserts.receiveAsFlow()
    private var job: Job? = null

    /** Which upload speaks for the state: a cancelled one's late end must not overwrite a newer one's. */
    private var generation = 0

    /** Puts the strip back from [UploadState.AlreadyUploading]. */
    private var already: Job? = null

    /**
     * Starts an upload of what [prepare] makes. While one is running, nothing starts: the strip says
     * [ALREADY_UPLOADING] for a moment (no image is dropped without a word, whichever source it came
     * from), and the result is false, so the caller gives back what it holds for it (a keyboard's grant).
     * Once taken, [prepare] always runs, even when the upload is cancelled before it began, so it can give
     * back its own. Call on the main thread, like [cancel] and [dismiss].
     */
    @OptIn(DelicateCoroutinesApi::class)
    fun start(prepare: suspend () -> PreparedImage): Boolean {
        if (job?.isActive == true) {
            val run = generation
            mutableState.value = UploadState.AlreadyUploading
            already?.cancel()
            already = scope.launch {
                delay(ALREADY_SHOWN)
                if (run == generation && mutableState.value == UploadState.AlreadyUploading) mutableState.value = UploadState.Uploading
            }
            return false
        }
        val run = ++generation
        mutableState.value = UploadState.Uploading
        // Atomic: a cancel before it is dispatched still runs `prepare` (which then stops at once).
        job = scope.launch(start = CoroutineStart.ATOMIC) {
            val outcome: UploadState = try {
                val image = prepare()
                val path = upload(image.bytes, image.format.extension)
                if (insertablePath(path)) {
                    if (run == generation) inserts.trySend(path)
                    UploadState.Idle
                } else {
                    UploadState.Failed(UNUSABLE_PATH)
                }
            } catch (error: CancellationException) {
                if (run == generation) mutableState.value = UploadState.Idle
                throw error
            } catch (error: Exception) {
                UploadState.Failed(uploadErrorMessage(error))
            }
            if (run == generation) mutableState.value = outcome
        }
        return true
    }

    /** Stops the running upload; nothing is inserted. */
    fun cancel() {
        generation++
        already?.cancel()
        job?.cancel()
        job = null
        mutableState.value = UploadState.Idle
    }

    /** Clears a failure from the strip. */
    fun dismiss() {
        if (mutableState.value is UploadState.Failed) mutableState.value = UploadState.Idle
    }
}

/** The state of a terminal without an image paste: nothing ever uploads. */
val NO_UPLOAD: StateFlow<UploadState> = MutableStateFlow<UploadState>(UploadState.Idle).asStateFlow()

/** The notice strip's words for a failed upload: the reason, never a path. */
fun uploadErrorMessage(error: Exception): String = when (error) {
    is ImageRefused -> error.message ?: UNREADABLE
    is HostException.SftpUnavailable -> "SFTP is not available on this host"
    is HostException.TooLarge -> TOO_LARGE
    is HostException.NotConnected, is HostException.Closed -> "Not sent: the host is not connected"
    is HostException.CommandFailed -> "Upload failed: ${error.reason}"
    is HostException -> "Upload failed"
    else -> "Upload failed"
}

/** What the terminal card's notice strip says about an upload, with its action ([UploadNotice.action]). */
data class UploadNotice(val notice: TerminalNotice, val action: String)

fun uploadNotice(state: UploadState): UploadNotice? = when (state) {
    UploadState.Idle -> null
    UploadState.Uploading -> UploadNotice(TerminalNotice("Uploading image…", NoticeTone.Info, busy = true, closable = false), "Cancel")
    UploadState.AlreadyUploading -> UploadNotice(TerminalNotice(ALREADY_UPLOADING, NoticeTone.Info, busy = true, closable = false), "Cancel")
    is UploadState.Failed -> UploadNotice(TerminalNotice(state.reason, NoticeTone.Warning, busy = false, closable = false), "Dismiss")
}
