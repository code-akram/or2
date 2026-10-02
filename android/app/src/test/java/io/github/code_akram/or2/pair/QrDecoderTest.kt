package io.github.code_akram.or2.pair

import com.google.zxing.BarcodeFormat
import com.google.zxing.EncodeHintType
import com.google.zxing.common.BitMatrix
import com.google.zxing.qrcode.QRCodeWriter
import com.google.zxing.qrcode.decoder.ErrorCorrectionLevel
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Test
import java.util.Random

/** The camera path's decoder over synthetic frames: ZXing's own writer makes the codes, the decoder reads luminance. */
class QrDecoderTest {
    private val code = "or2-pair:2?name=Work%20Mac&user=alice&port=22&a=192.168.1.20&a=work-mac.local" +
        "&hk=ssh-ed25519%20AAAAC3NzaC1lZDI1NTE5AAAAIAc39XUWT33SvSLy6vA7I83%2BXgmwnHmYtMQRjLeaZ2U7" +
        "&id=abcdefghijklm"

    private fun matrix(text: String, size: Int, level: ErrorCorrectionLevel = ErrorCorrectionLevel.L): BitMatrix =
        QRCodeWriter().encode(
            text, BarcodeFormat.QR_CODE, size, size,
            mapOf(EncodeHintType.ERROR_CORRECTION to level, EncodeHintType.MARGIN to 4, EncodeHintType.CHARACTER_SET to "UTF-8"),
        )

    /** Dark modules are 0, light 255, like a camera's Y plane; [stride] pads each row as ImageProxy planes do. */
    private fun luminance(m: BitMatrix, stride: Int = m.width, invert: Boolean = false): ByteArray {
        val bytes = ByteArray(stride * m.height) { 128.toByte() }
        for (y in 0 until m.height) for (x in 0 until m.width) {
            val dark = m[x, y] != invert
            bytes[y * stride + x] = if (dark) 0 else 255.toByte()
        }
        return bytes
    }

    @Test
    fun readsAPairingCodeFromAFrame() {
        val m = matrix(code, 400)
        assertEquals(code, QrDecoder().decode(luminance(m), m.width, m.height))
    }

    @Test
    fun honoursARowStrideWiderThanTheImage() {
        val m = matrix(code, 400)
        assertEquals(code, QrDecoder().decode(luminance(m, stride = m.width + 24), m.width, m.height, m.width + 24))
    }

    @Test
    fun readsALightOnDarkCodeToo() {
        val m = matrix(code, 400)
        assertEquals(code, QrDecoder().decode(luminance(m, invert = true), m.width, m.height))
    }

    @Test
    fun readsACodeTurnedOnItsSide() {
        val m = matrix(code, 400)
        val side = BitMatrix(m.height, m.width)
        for (y in 0 until m.height) for (x in 0 until m.width) if (m[x, y]) side.set(y, x)
        assertEquals(code, QrDecoder().decode(luminance(side), side.width, side.height))
    }

    @Test
    fun readsAFullKilobyteCode() {
        val long = "or2-pair:2?" + "a=x".repeat(1) + "&hk=" + "A".repeat(1000)
        val m = matrix(long, 900)
        assertEquals(long, QrDecoder().decode(luminance(m), m.width, m.height))
    }

    @Test
    fun readsACodeThroughSensorNoise() {
        val m = matrix(code, 500, ErrorCorrectionLevel.M)
        val bytes = luminance(m)
        val random = Random(7)
        for (i in bytes.indices) {
            val value = (bytes[i].toInt() and 0xff) + random.nextInt(61) - 30
            bytes[i] = value.coerceIn(0, 255).toByte()
        }
        assertEquals(code, QrDecoder().decode(bytes, m.width, m.height))
    }

    @Test
    fun aFrameWithoutACodeIsNull() {
        val random = Random(1)
        val noise = ByteArray(320 * 240).also(random::nextBytes)
        val decoder = QrDecoder()
        assertNull(decoder.decode(noise, 320, 240))
        assertNull(decoder.decode(ByteArray(320 * 240) { 200.toByte() }, 320, 240))
        // The decoder still works after misses (state is reset each time).
        val m = matrix(code, 400)
        assertEquals(code, decoder.decode(luminance(m), m.width, m.height))
    }

    @Test
    fun anotherKindOfQrCodeIsReadAsTextAndLeftToTheParserToRefuse() {
        val m = matrix("https://example.org/", 300)
        assertEquals("https://example.org/", QrDecoder().decode(luminance(m), m.width, m.height))
    }

    @Test
    fun inconsistentGeometryIsRefused() {
        val decoder = QrDecoder()
        assertThrows(IllegalArgumentException::class.java) { decoder.decode(ByteArray(10), 20, 20) }
        assertThrows(IllegalArgumentException::class.java) { decoder.decode(ByteArray(400), 20, 20, 10) }
        assertThrows(IllegalArgumentException::class.java) { decoder.decode(ByteArray(400), 0, 20) }
    }
}
