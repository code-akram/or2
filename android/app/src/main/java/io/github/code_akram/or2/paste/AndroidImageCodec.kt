package io.github.code_akram.or2.paste

import android.content.ContentResolver
import android.content.Context
import android.content.res.AssetFileDescriptor
import android.graphics.Bitmap
import android.graphics.ImageDecoder
import android.net.Uri
import android.os.CancellationSignal
import java.io.ByteArrayOutputStream
import java.io.InputStream
import java.nio.ByteBuffer

/**
 * [ImageCodec] on the phone: `ImageDecoder` (PNG, JPEG, WebP, GIF's first frame, HEIF; EXIF
 * orientation applied) scaling while it decodes, into a software bitmap; `Bitmap.compress`, which
 * writes no metadata.
 */
object AndroidImageCodec : ImageCodec<Bitmap> {
    override fun decode(bytes: ByteArray, target: (width: Int, height: Int) -> PixelSize): Decoded<Bitmap> {
        var mime: String? = null
        val bitmap = ImageDecoder.decodeBitmap(ImageDecoder.createSource(ByteBuffer.wrap(bytes))) { decoder, info, _ ->
            mime = info.mimeType
            val size = target(info.size.width, info.size.height)
            if (size.width != info.size.width || size.height != info.size.height) decoder.setTargetSize(size.width, size.height)
            decoder.allocator = ImageDecoder.ALLOCATOR_SOFTWARE
        }
        return Decoded(bitmap, mime)
    }

    override fun hasTransparency(image: Bitmap): Boolean {
        if (!image.hasAlpha()) return false
        // A format that can carry alpha is not the same as an image that uses it.
        val row = IntArray(image.width)
        for (y in 0 until image.height) {
            image.getPixels(row, 0, image.width, 0, y, image.width, 1)
            if (row.any { it ushr 24 != 0xff }) return true
        }
        return false
    }

    override fun encode(image: Bitmap, format: ImageFormat, quality: Int): ByteArray {
        val out = ByteArrayOutputStream()
        val compressed = when (format) {
            ImageFormat.PNG -> image.compress(Bitmap.CompressFormat.PNG, 100, out)
            ImageFormat.JPEG -> image.compress(Bitmap.CompressFormat.JPEG, quality, out)
        }
        if (!compressed) throw ImageRefused(UNREADABLE)
        return out.toByteArray()
    }

    override fun release(image: Bitmap) = image.recycle()
}

/**
 * The upload's preparation for an image at [uri] (the Photo Picker, a keyboard's content, a share), by
 * [imagePreparation]: only a `content:` URI is read ([readableImageScheme]), at most 20 MiB within
 * [READ_TIMEOUT], and a cancel or the timeout stops a provider that does not answer ([AndroidImageSource]).
 * [release] (a keyboard's read grant) runs exactly once, as soon as the reading is over, whichever way.
 */
fun imageFromUri(context: Context, uri: Uri, release: () -> Unit = {}): suspend () -> PreparedImage {
    val resolver = context.applicationContext.contentResolver
    return imagePreparation(uri.scheme, { AndroidImageSource(resolver, uri) }, release, AndroidImageCodec)
}

/**
 * A `content:` image read through an [AssetFileDescriptor] opened with a [CancellationSignal]: [abort]
 * cancels a pending open (the provider is told) and closes the descriptor, which wakes a read blocked on it
 * (Android signals the threads blocked on a descriptor it closes). A provider that ignores the cancel keeps
 * one IO thread until it answers; the upload has ended by then and its grant was given back.
 */
class AndroidImageSource(private val resolver: ContentResolver, private val uri: Uri) : ImageSource {
    private val signal = CancellationSignal()
    private var descriptor: AssetFileDescriptor? = null
    private var aborted = false

    override fun open(): InputStream {
        val opened = resolver.openAssetFileDescriptor(uri, "r", signal) ?: throw ImageRefused(UNREADABLE)
        synchronized(this) {
            if (aborted) {
                closeQuietly(opened)
                throw ImageRefused(UNREADABLE)
            }
            descriptor = opened
        }
        return opened.createInputStream()
    }

    override fun abort() {
        val opened = synchronized(this) {
            if (aborted) return
            aborted = true
            descriptor
        }
        signal.cancel()
        opened?.let(::closeQuietly)
    }

    private fun closeQuietly(opened: AssetFileDescriptor) {
        try {
            opened.close()
        } catch (_: Exception) {
        }
    }
}
