package io.github.code_akram.or2.paste

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.async
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import java.io.InputStream
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.coroutines.cancellation.CancellationException
import kotlin.time.Duration
import kotlin.time.Duration.Companion.seconds

/**
 * Where an image is read from (the Photo Picker, a keyboard, a share: another app's content provider), in a
 * way that can be stopped. A provider may never answer an open or a read; [abort] makes both give up.
 */
interface ImageSource {
    /** Opens the image for reading; may block until it answers or [abort] is called. */
    fun open(): InputStream

    /** Stops a blocked [open] or read, and closes what was opened. From any thread, at once, more than once. */
    fun abort()
}

/** How long reading an image may take from its open to its last byte (a cloud photo is fetched first). */
val READ_TIMEOUT: Duration = 60.seconds

/**
 * Whether an image at a URI of [scheme] is read: only `content:`, what the Photo Picker, keyboards and
 * shares hand over, each under its own read grant. A `file:` URI (or none) would be opened with or2's own
 * rights, so another app could have or2 read and upload a file that only or2 may read.
 */
fun readableImageScheme(scheme: String?): Boolean = scheme == "content"

/** Runs [action] once, however many times it is asked to, from whatever thread. */
class Once(private val action: () -> Unit) {
    private val done = AtomicBoolean(false)

    operator fun invoke() {
        if (done.compareAndSet(false, true)) action()
    }
}

/** Blocking reads run here, outside any caller's job: a cancelled caller never waits for one. */
private val readers = CoroutineScope(SupervisorJob() + Dispatchers.IO)

/**
 * Reads [source] whole (at most [MAX_IMAGE_BYTES], [readCapped]) within [timeout]. The read runs on its own
 * IO thread: a caller that is cancelled, or the timeout, [ImageSource.abort]s it and returns at once, even
 * while the provider has not answered. Throws [ImageRefused] ([TOO_SLOW] past the timeout).
 */
suspend fun readImage(source: ImageSource, timeout: Duration = READ_TIMEOUT): ByteArray {
    val reading = readers.async { source.open().use { readCapped(it) } }
    try {
        return withTimeout(timeout) { reading.await() }
    } catch (_: TimeoutCancellationException) {
        throw ImageRefused(TOO_SLOW)
    } catch (error: CancellationException) {
        throw error
    } catch (error: ImageRefused) {
        throw error
    } catch (_: Exception) {
        throw ImageRefused(UNREADABLE)
    } finally {
        if (!reading.isCompleted) {
            source.abort()
            reading.cancel()
        }
    }
}

/**
 * An upload's preparation of the image at a URI of [scheme] (see [readableImageScheme]): [source] read
 * ([readImage]), then processed on the default dispatcher ([prepareImage]). [release] (a keyboard's read
 * grant) runs exactly once, as soon as the reading is over, whichever way: read, refused, timed out or
 * cancelled. The caller must run the result at least once ([ImagePaste] always does, even cancelled).
 */
fun <I> imagePreparation(
    scheme: String?, source: () -> ImageSource, release: () -> Unit, codec: ImageCodec<I>, timeout: Duration = READ_TIMEOUT,
): suspend () -> PreparedImage {
    val released = Once(release)
    return {
        val bytes = try {
            if (!readableImageScheme(scheme)) throw ImageRefused(UNREADABLE)
            readImage(source(), timeout)
        } finally {
            released()
        }
        withContext(Dispatchers.Default) { prepareImage(bytes, codec) }
    }
}

const val TOO_SLOW = "The image took too long to read"
