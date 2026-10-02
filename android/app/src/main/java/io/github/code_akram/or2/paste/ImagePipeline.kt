package io.github.code_akram.or2.paste

import java.io.ByteArrayOutputStream
import java.io.InputStream

/**
 * What an image goes through before it is uploaded (contracts.md, "Image paste"): it is decoded,
 * scaled so its long edge is at most [MAX_EDGE] px, and encoded again, PNG when it has
 * transparency or is a PNG under [PNG_KEEP_BYTES] (a screenshot: text stays sharp), else JPEG at
 * [JPEG_QUALITY]. Encoding again is also what drops EXIF and GPS: nothing of the original file is
 * sent, not even for an image that needs no scaling. A source above [MAX_IMAGE_BYTES] is refused
 * before it is decoded, as is a result above it.
 *
 * The decisions are here, on plain values; the pixels go through an [ImageCodec] ([AndroidImageCodec]
 * on the phone, an `ImageIO` one in the JVM tests).
 */
const val MAX_EDGE = 2048
const val MAX_IMAGE_BYTES = 20 * 1024 * 1024
const val PNG_KEEP_BYTES = 2 * 1024 * 1024
const val JPEG_QUALITY = 85

enum class ImageFormat(val extension: String) { PNG("png"), JPEG("jpg") }

/** The bytes to upload and their format (its file extension names it on the host). */
class PreparedImage(val bytes: ByteArray, val format: ImageFormat)

/** Why an image is not uploaded, in words for the notice strip. */
class ImageRefused(message: String) : Exception(message)

data class PixelSize(val width: Int, val height: Int)

/** A decoded image and what its source said about itself. */
class Decoded<I>(val image: I, val mimeType: String?)

/** Decoding, inspecting and encoding pixels; the pipeline decides everything else. */
interface ImageCodec<I> {
    /**
     * Decodes [bytes] at the size [target] returns for the source's own width and height (the codec
     * scales while decoding, so a large photo is never held at full size). Throws on an unreadable image.
     */
    fun decode(bytes: ByteArray, target: (width: Int, height: Int) -> PixelSize): Decoded<I>

    /** Whether any pixel is not fully opaque. */
    fun hasTransparency(image: I): Boolean

    /** [format] (JPEG at [quality]); no metadata. */
    fun encode(image: I, format: ImageFormat, quality: Int): ByteArray

    fun release(image: I) {}
}

/** [width] x [height] scaled down (never up) so the long edge is at most [maxEdge], aspect kept. */
fun fitWithin(width: Int, height: Int, maxEdge: Int = MAX_EDGE): PixelSize {
    require(width > 0 && height > 0) { "an image has a size" }
    val long = maxOf(width, height)
    if (long <= maxEdge) return PixelSize(width, height)
    val scale = maxEdge.toDouble() / long
    return PixelSize(
        if (width >= height) maxEdge else maxOf(1, Math.round(width * scale).toInt()),
        if (height > width) maxEdge else maxOf(1, Math.round(height * scale).toInt()),
    )
}

/** PNG for transparency or a small PNG (a screenshot), JPEG for everything else. */
fun chooseFormat(sourceMime: String?, sourceBytes: Int, transparent: Boolean): ImageFormat = when {
    transparent -> ImageFormat.PNG
    sourceMime.equals("image/png", ignoreCase = true) && sourceBytes < PNG_KEEP_BYTES -> ImageFormat.PNG
    else -> ImageFormat.JPEG
}

/** Decodes, scales and encodes [source] (see the file comment). Throws [ImageRefused]. */
fun <I> prepareImage(source: ByteArray, codec: ImageCodec<I>): PreparedImage {
    if (source.size > MAX_IMAGE_BYTES) throw ImageRefused(TOO_LARGE)
    if (source.isEmpty()) throw ImageRefused(UNREADABLE)
    val decoded = try {
        codec.decode(source) { width, height -> fitWithin(width, height) }
    } catch (error: ImageRefused) {
        throw error
    } catch (_: Exception) {
        throw ImageRefused(UNREADABLE)
    }
    try {
        val format = chooseFormat(decoded.mimeType, source.size, codec.hasTransparency(decoded.image))
        val bytes = codec.encode(decoded.image, format, JPEG_QUALITY)
        if (bytes.size > MAX_IMAGE_BYTES) throw ImageRefused(TOO_LARGE)
        if (bytes.isEmpty()) throw ImageRefused(UNREADABLE)
        return PreparedImage(bytes, format)
    } finally {
        codec.release(decoded.image)
    }
}

/** Reads [input] whole, refusing ([ImageRefused]) as soon as it is longer than [cap]. */
fun readCapped(input: InputStream, cap: Int = MAX_IMAGE_BYTES): ByteArray {
    val out = ByteArrayOutputStream()
    val buffer = ByteArray(64 * 1024)
    while (true) {
        val read = input.read(buffer)
        if (read < 0) break
        if (out.size() + read > cap) throw ImageRefused(TOO_LARGE)
        out.write(buffer, 0, read)
    }
    return out.toByteArray()
}

const val TOO_LARGE = "The image is larger than 20 MiB"
const val UNREADABLE = "The image cannot be read"
