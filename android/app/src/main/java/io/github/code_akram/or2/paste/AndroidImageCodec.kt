package io.github.code_akram.or2.paste

import android.content.Context
import android.graphics.Bitmap
import android.graphics.ImageDecoder
import android.net.Uri
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.ByteArrayOutputStream
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
 * The upload's preparation for an image at [uri] (the Photo Picker, a keyboard's content, a share):
 * read (at most 20 MiB) on the IO dispatcher, processed on the default one. [release] runs once it was
 * read, whatever happened (a keyboard's read permission is given back).
 */
fun imageFromUri(context: Context, uri: Uri, release: () -> Unit = {}): suspend () -> PreparedImage = {
    val bytes = withContext(Dispatchers.IO) {
        try {
            context.contentResolver.openInputStream(uri)?.use { readCapped(it) } ?: throw ImageRefused(UNREADABLE)
        } catch (error: ImageRefused) {
            throw error
        } catch (_: Exception) {
            throw ImageRefused(UNREADABLE)
        } finally {
            release()
        }
    }
    withContext(Dispatchers.Default) { prepareImage(bytes, AndroidImageCodec) }
}
