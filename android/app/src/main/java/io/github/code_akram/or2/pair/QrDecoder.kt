package io.github.code_akram.or2.pair

import com.google.zxing.BarcodeFormat
import com.google.zxing.BinaryBitmap
import com.google.zxing.ChecksumException
import com.google.zxing.DecodeHintType
import com.google.zxing.FormatException
import com.google.zxing.LuminanceSource
import com.google.zxing.NotFoundException
import com.google.zxing.PlanarYUVLuminanceSource
import com.google.zxing.common.HybridBinarizer
import com.google.zxing.qrcode.QRCodeReader

/**
 * Reads a QR code from a camera frame's luminance (the Y plane of a YUV_420_888 image), with ZXing
 * core (Apache-2.0, pure Java: no Google Play Services, no ML Kit). Pure and thread-confined: make
 * one per analysis thread. A terminal QR drawn light-on-dark is read too, by a second attempt on
 * the inverted image (some terminals draw the code that way).
 */
class QrDecoder {
    private val reader = QRCodeReader()
    private val hints = mapOf(
        DecodeHintType.POSSIBLE_FORMATS to listOf(BarcodeFormat.QR_CODE),
        DecodeHintType.TRY_HARDER to true,
        DecodeHintType.CHARACTER_SET to "UTF-8",
    )

    /**
     * The text of the code in the frame, or null when there is none (or it is unreadable).
     * [luminance] holds [height] rows of [rowStride] bytes, of which the first [width] are pixels.
     */
    fun decode(luminance: ByteArray, width: Int, height: Int, rowStride: Int = width): String? {
        require(width > 0 && height > 0 && rowStride >= width && luminance.size >= rowStride * (height - 1) + width) {
            "inconsistent frame geometry"
        }
        val source = PlanarYUVLuminanceSource(luminance, rowStride, height, 0, 0, width, height, false)
        return attempt(source) ?: attempt(source.invert())
    }

    private fun attempt(source: LuminanceSource): String? = try {
        reader.decode(BinaryBitmap(HybridBinarizer(source)), hints).text
    } catch (_: NotFoundException) {
        null
    } catch (_: ChecksumException) {
        null
    } catch (_: FormatException) {
        null
    } finally {
        reader.reset()
    }
}
