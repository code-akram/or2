package io.github.code_akram.or2.paste

import io.github.code_akram.or2.ffi.HostException
import io.github.code_akram.or2.session.TerminalNotice
import io.github.code_akram.or2.ui.NoticeTone
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.DelicateCoroutinesApi
import kotlinx.coroutines.Job
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.launch

/** One terminal's image uploads, as its notice strip shows them. */
sealed interface UploadState {
    data object Idle : UploadState

    /**
     * Uploading the [image]th of the [images] the queue has taken in this run ([images] grows while more join).
     * [full]: an image beyond [MAX_IMAGES] just came and was not taken; the strip says so for [ALREADY_SHOWN].
     */
    data class Uploading(val image: Int = 1, val images: Int = 1, val full: Boolean = false) : UploadState
    data class Failed(val reason: String) : UploadState
}

/** Whether an upload runs (Cancel stops it). */
val UploadState.uploading: Boolean get() = this is UploadState.Uploading

/** How many images a terminal's queue holds at once, pending or uploading. */
const val MAX_IMAGES = 10

/** How long the strip says an image beyond [MAX_IMAGES] was not taken. */
const val ALREADY_SHOWN = 3_000L

const val TOO_MANY_IMAGES = "At most $MAX_IMAGES images at a time"

/** What a host's answer that is no path to type into a terminal fails with ([insertablePath]). */
const val UNUSABLE_PATH = "Upload failed: the host answered an unusable path"

/**
 * A terminal's image paste (contracts.md, "Image paste", "Several images at once"): one queue, fed by every
 * source (the composer's attach button, a keyboard's image, a share) in arrival order, that uploads one image
 * at a time. [start] adds an image; the queue prepares it ([prepareImage], off the main thread) and uploads it
 * over the host's connection ([upload]: `upload_image`). A failed image is skipped and the rest still upload.
 * When the queue is empty, the paths that uploaded arrive together on [paths] for the terminal screen to
 * insert as one ([insertTarget], [pathsInsertion]), once, even when the screen was not showing at that moment.
 * [cancel] stops the running upload (its coroutine is cancelled, and Rust removes the temporary file), drops
 * what is still queued and inserts nothing. It lives with the terminal in the holder's [scope], not with a view.
 */
class ImagePaste(
    private val scope: CoroutineScope,
    private val upload: suspend (bytes: ByteArray, extension: String) -> String,
) {
    private val mutableState = MutableStateFlow<UploadState>(UploadState.Idle)
    val state: StateFlow<UploadState> = mutableState.asStateFlow()
    private val inserts = Channel<List<String>>(Channel.UNLIMITED)

    /** Each finished queue run's uploaded paths, in arrival order, delivered once (a cancelled run delivers none). */
    val paths: Flow<List<String>> = inserts.receiveAsFlow()

    /** One run of the queue: from its first image until it is empty again (or cancelled). */
    private class Run(val generation: Int) {
        /** The images taken and not yet started, each its preparation. */
        val waiting = ArrayDeque<suspend () -> PreparedImage>()

        /** The images taken in this run. */
        var images = 0

        /** The images that ended, uploaded or failed. */
        var done = 0
        val paths = mutableListOf<String>()
        val failures = mutableListOf<String>()
    }

    private var run: Run? = null
    private var job: Job? = null

    /** Which run speaks for the state: a cancelled one's late end must not overwrite a newer one's. */
    private var generation = 0

    /** Whether the strip says [TOO_MANY_IMAGES], and what puts it back. */
    private var full = false
    private var fullShown: Job? = null

    /**
     * Adds an image, made by [prepare], to the queue, starting it when it is not running. With [MAX_IMAGES]
     * pending or uploading, nothing is taken: the strip says [TOO_MANY_IMAGES] for a moment (no image is
     * dropped without a word, whichever source it came from), and the result is false, so the caller gives back
     * what it holds for it (a keyboard's grant). Once taken, [prepare] always runs, even when the queue is
     * cancelled before it reached the image, so it can give back its own. Call on the main thread, like
     * [cancel] and [dismiss].
     */
    @OptIn(DelicateCoroutinesApi::class)
    fun start(prepare: suspend () -> PreparedImage): Boolean {
        val running = run
        if (running != null && running.images - running.done >= MAX_IMAGES) {
            full = true
            show(running)
            fullShown?.cancel()
            fullShown = scope.launch {
                delay(ALREADY_SHOWN)
                full = false
                if (run === running) show(running)
            }
            return false
        }
        val queue = running ?: Run(++generation)
        queue.waiting.addLast(prepare)
        queue.images++
        if (running == null) {
            run = queue
            // Atomic: a cancel before it is dispatched still runs every `prepare` (each then stops at once).
            job = scope.launch(start = CoroutineStart.ATOMIC) { work(queue) }
        }
        show(queue)
        return true
    }

    /** Uploads [queue]'s images one at a time until none is waiting, then delivers its paths and its outcome. */
    private suspend fun work(queue: Run) {
        try {
            while (true) {
                val prepare = queue.waiting.removeFirstOrNull() ?: break
                try {
                    val image = prepare()
                    // A run cancelled while the image was prepared uploads nothing of it.
                    currentCoroutineContext().ensureActive()
                    val path = upload(image.bytes, image.format.extension)
                    if (insertablePath(path)) queue.paths += path else queue.failures += UNUSABLE_PATH
                } catch (error: CancellationException) {
                    throw error
                } catch (error: Exception) {
                    queue.failures += uploadErrorMessage(error)
                }
                queue.done++
                if (queue.waiting.isNotEmpty()) show(queue)
            }
        } catch (error: CancellationException) {
            // Every taken image is prepared, so each gives back what it holds; cancelled, each stops at once.
            while (true) {
                val prepare = queue.waiting.removeFirstOrNull() ?: break
                try {
                    prepare()
                } catch (_: Exception) {
                    // Cancelled, so it stopped; what it would have made is not wanted.
                }
            }
            if (queue.generation == generation) end(UploadState.Idle)
            throw error
        }
        if (queue.generation != generation) return
        if (queue.paths.isNotEmpty()) inserts.trySend(queue.paths.toList())
        end(queueFailure(queue.failures, queue.images)?.let { UploadState.Failed(it) } ?: UploadState.Idle)
    }

    private fun show(queue: Run) {
        if (queue.generation == generation) mutableState.value = UploadState.Uploading(queue.done + 1, queue.images, full)
    }

    private fun end(outcome: UploadState) {
        run = null
        job = null
        full = false
        fullShown?.cancel()
        mutableState.value = outcome
    }

    /** Stops the running upload and drops the queued images; nothing of this run is inserted. */
    fun cancel() {
        generation++
        val running = job
        end(UploadState.Idle)
        running?.cancel()
    }

    /** Clears a failure from the strip. */
    fun dismiss() {
        if (mutableState.value is UploadState.Failed) mutableState.value = UploadState.Idle
    }
}

/**
 * What the strip says when a queue run of [images] ended with [failures] (their reasons, in order), or null
 * when none failed: one image, its reason; several, how many failed and the first reason.
 */
fun queueFailure(failures: List<String>, images: Int): String? = when {
    failures.isEmpty() -> null
    images == 1 -> failures.first()
    else -> "${failures.size} of $images images failed: ${failures.first()}"
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

/**
 * What the terminal card's notice strip says about an upload: its progress (muted, a spinner, **Cancel**) or why it
 * failed (a warning, **Dismiss**); nothing while the queue is idle.
 */
fun uploadNotice(state: UploadState): TerminalNotice? = when (state) {
    UploadState.Idle -> null
    is UploadState.Uploading -> TerminalNotice(uploadingText(state), NoticeTone.Info, busy = true, action = "Cancel")
    is UploadState.Failed -> TerminalNotice(state.reason, NoticeTone.Warning, busy = false, action = "Dismiss")
}

/** `Uploading image…` for one image, `Uploading image <i> of <n>…` for several, or the moment's [TOO_MANY_IMAGES]. */
private fun uploadingText(state: UploadState.Uploading): String = when {
    state.full -> TOO_MANY_IMAGES
    state.images == 1 -> "Uploading image…"
    else -> "Uploading image ${state.image} of ${state.images}…"
}
