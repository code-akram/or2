package io.github.code_akram.or2.paste

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.DataInputStream
import java.io.DataOutputStream
import java.io.InputStream
import java.util.zip.CRC32
import java.util.zip.DeflaterOutputStream
import java.util.zip.InflaterInputStream

/**
 * The image processing of contracts.md, "Image paste", on the JVM (no Android graphics, no AWT): the
 * pipeline's decisions with real pixels through a small codec of the test's own. It reads and writes
 * real PNG (8-bit RGB or RGBA, every filter), and a stand-in for JPEG that keeps JPEG's container (SOI,
 * APP segments such as EXIF, EOI) around raw pixels, so metadata in a source is really there to leak.
 * The phone's codec is `ImageDecoder` and `Bitmap.compress`, which, like this one, write pixels only.
 */
class ImagePipelineTest {
    class Raster(val width: Int, val height: Int, val argb: IntArray = IntArray(width * height))

    private object TestCodec : ImageCodec<Raster> {
        val decodedAt = mutableListOf<PixelSize>()

        override fun decode(bytes: ByteArray, target: (width: Int, height: Int) -> PixelSize): Decoded<Raster> {
            val (source, mime) = when {
                bytes.isPng() -> readPng(bytes) to "image/png"
                bytes.isJpeg() -> readJpeg(bytes) to "image/jpeg"
                else -> error("unknown image")
            }
            val size = target(source.width, source.height)
            decodedAt += size
            // Nearest neighbour is enough to see the size.
            val scaled = Raster(size.width, size.height)
            for (y in 0 until size.height) for (x in 0 until size.width) {
                scaled.argb[y * size.width + x] =
                    source.argb[(y * source.height / size.height) * source.width + x * source.width / size.width]
            }
            return Decoded(scaled, mime)
        }

        override fun hasTransparency(image: Raster) = image.argb.any { it ushr 24 != 0xff }

        override fun encode(image: Raster, format: ImageFormat, quality: Int): ByteArray = when (format) {
            ImageFormat.PNG -> writePng(image)
            ImageFormat.JPEG -> writeJpeg(image, exif = null)
        }
    }

    private fun raster(width: Int, height: Int, transparentBand: Boolean = false) = Raster(width, height).apply {
        // Sixteen bands, one of them transparent when asked: PNG compresses it to almost nothing.
        for (y in 0 until height) {
            val band = y * 16 / height
            val alpha = if (transparentBand && band == 3) 0 else 0xff
            val colour = (alpha shl 24) or ((band * 15) shl 16) or ((255 - band * 15) shl 8) or (band * 37 % 255)
            for (x in 0 until width) argb[y * width + x] = colour
        }
    }

    @Test
    fun theLongEdgeIsScaledTo2048AndTheAspectKept() {
        assertEquals(PixelSize(2048, 1536), fitWithin(4000, 3000))
        assertEquals(PixelSize(1536, 2048), fitWithin(3000, 4000))
        assertEquals(PixelSize(2048, 2048), fitWithin(5000, 5000))
        assertEquals(PixelSize(2048, 1), fitWithin(10_000, 2))
        assertEquals(PixelSize(922, 2048), fitWithin(1080, 2400))
        // Never scaled up.
        assertEquals(PixelSize(800, 600), fitWithin(800, 600))
        assertEquals(PixelSize(2048, 100), fitWithin(2048, 100))

        val large = prepareImage(writeJpeg(raster(3000, 1000), exif = null), TestCodec)
        assertEquals(PixelSize(2048, 683), TestCodec.decodedAt.last())
        val result = readJpeg(large.bytes)
        assertEquals(2048, result.width)
        assertEquals(683, result.height)
    }

    @Test
    fun reencodingDropsExifAndGpsEvenWhenNothingIsScaled() {
        val photo = writeJpeg(raster(64, 48), exif = "Exif\u0000\u0000GPSLatitude=or2-secret-location")
        assertTrue(photo.has("Exif") && photo.has("or2-secret-location"))
        val prepared = prepareImage(photo, TestCodec)
        assertEquals(ImageFormat.JPEG, prepared.format)
        assertTrue(prepared.bytes.isJpeg())
        assertFalse("no EXIF", prepared.bytes.has("Exif"))
        assertFalse("no GPS", prepared.bytes.has("or2-secret-location"))
        assertEquals(64, readJpeg(prepared.bytes).width)
        // The same for a PNG with a text chunk: its pixels go, its metadata does not.
        val png = writePng(raster(32, 32), text = "GPS or2-secret-location")
        assertTrue(png.has("or2-secret-location"))
        val small = prepareImage(png, TestCodec)
        assertEquals(ImageFormat.PNG, small.format)
        assertFalse(small.bytes.has("or2-secret-location"))
        assertArrayEquals(readPng(png).argb, readPng(small.bytes).argb)
    }

    @Test
    fun transparencyAndSmallPngsStayPngEverythingElseIsJpeg() {
        // A screenshot-sized PNG under 2 MiB stays PNG (sharp text), scaled to 2048 high.
        val screenshot = prepareImage(writePng(raster(1080, 2400)), TestCodec)
        assertEquals(ImageFormat.PNG, screenshot.format)
        assertEquals("png", screenshot.format.extension)
        assertTrue(screenshot.bytes.isPng())
        assertEquals(2048, readPng(screenshot.bytes).height)
        // Transparency stays PNG whatever the source.
        val transparent = prepareImage(writePng(raster(300, 300, transparentBand = true)), TestCodec)
        assertEquals(ImageFormat.PNG, transparent.format)
        // A JPEG photo is JPEG.
        val photo = prepareImage(writeJpeg(raster(400, 300), exif = null), TestCodec)
        assertEquals(ImageFormat.JPEG, photo.format)
        assertEquals("jpg", photo.format.extension)
        // An opaque PNG of 2 MiB or more is a photo: JPEG.
        val noisy = Raster(1024, 700).apply { var seed = 1; for (i in argb.indices) { seed = seed * 1_103_515_245 + 12_345; argb[i] = seed or (0xff shl 24) } }
        val big = writePng(noisy)
        assertTrue(big.size >= PNG_KEEP_BYTES)
        assertEquals(ImageFormat.JPEG, prepareImage(big, TestCodec).format)
        // The rule itself.
        assertEquals(ImageFormat.JPEG, chooseFormat("image/png", PNG_KEEP_BYTES, transparent = false))
        assertEquals(ImageFormat.PNG, chooseFormat("image/png", PNG_KEEP_BYTES - 1, transparent = false))
        assertEquals(ImageFormat.PNG, chooseFormat("image/webp", 10 * 1024 * 1024, transparent = true))
        assertEquals(ImageFormat.JPEG, chooseFormat("image/gif", 10, transparent = false))
        assertEquals(ImageFormat.JPEG, chooseFormat(null, 10, transparent = false))
    }

    @Test
    fun anImageAbove20MibIsRefusedBeforeItIsDecoded() {
        val decodes = TestCodec.decodedAt.size
        val refused = assertThrows(ImageRefused::class.java) { prepareImage(ByteArray(MAX_IMAGE_BYTES + 1), TestCodec) }
        assertEquals(TOO_LARGE, refused.message)
        assertEquals("nothing decoded", decodes, TestCodec.decodedAt.size)
        // Reading stops as soon as the cap is passed.
        var served = 0L
        val endless = object : InputStream() {
            override fun read(): Int = 0.also { served++ }
            override fun read(b: ByteArray, off: Int, len: Int): Int { served += len; return len }
        }
        assertThrows(ImageRefused::class.java) { readCapped(endless) }
        assertTrue(served <= MAX_IMAGE_BYTES + 64 * 1024)
        val bytes = ByteArray(100_000) { it.toByte() }
        assertArrayEquals(bytes, readCapped(ByteArrayInputStream(bytes)))
        assertArrayEquals(bytes, readCapped(ByteArrayInputStream(bytes), cap = bytes.size))
        assertThrows(ImageRefused::class.java) { readCapped(ByteArrayInputStream(bytes), cap = bytes.size - 1) }
    }

    @Test
    fun anUnreadableImageIsRefusedInWords() {
        val refused = assertThrows(ImageRefused::class.java) { prepareImage("not an image".toByteArray(), TestCodec) }
        assertEquals(UNREADABLE, refused.message)
        assertThrows(ImageRefused::class.java) { prepareImage(ByteArray(0), TestCodec) }
    }

    @Test
    fun theTestCodecReadsEveryPngFilter() {
        // Rows written with each of PNG's five filters read back exactly.
        val image = raster(7, 5, transparentBand = true).apply { argb[3] = 0x80123456.toInt() }
        for (filter in 0..4) assertArrayEquals("filter $filter", image.argb, readPng(writePng(image, filter = filter)).argb)
    }
}

// --- the test codec's formats ------------------------------------------------------------------

private fun ByteArray.isPng() = size > 8 && this[0] == 0x89.toByte() && this[1] == 'P'.code.toByte()
private fun ByteArray.isJpeg() = size > 3 && this[0] == 0xff.toByte() && this[1] == 0xd8.toByte()
private fun ByteArray.has(text: String) = String(this, Charsets.ISO_8859_1).contains(text)

private val PNG_SIGNATURE = byteArrayOf(0x89.toByte(), 'P'.code.toByte(), 'N'.code.toByte(), 'G'.code.toByte(), 13, 10, 26, 10)

/** RGBA, 8 bits, every row with [filter]; an optional `tEXt` chunk. */
private fun writePng(image: ImagePipelineTest.Raster, filter: Int = 0, text: String? = null): ByteArray {
    val out = ByteArrayOutputStream()
    val data = DataOutputStream(out)
    fun chunk(type: String, body: ByteArray) {
        data.writeInt(body.size)
        val typed = type.toByteArray(Charsets.ISO_8859_1) + body
        data.write(typed)
        data.writeInt(CRC32().apply { update(typed) }.value.toInt())
    }
    data.write(PNG_SIGNATURE)
    chunk("IHDR", ByteArrayOutputStream().also {
        DataOutputStream(it).apply { writeInt(image.width); writeInt(image.height); write(byteArrayOf(8, 6, 0, 0, 0)) }
    }.toByteArray())
    if (text != null) chunk("tEXt", "Comment\u0000$text".toByteArray(Charsets.ISO_8859_1))
    val stride = image.width * 4
    val raw = ByteArrayOutputStream()
    var previous = ByteArray(stride)
    for (y in 0 until image.height) {
        val row = ByteArray(stride)
        for (x in 0 until image.width) {
            val p = image.argb[y * image.width + x]
            row[x * 4] = (p shr 16).toByte(); row[x * 4 + 1] = (p shr 8).toByte(); row[x * 4 + 2] = p.toByte(); row[x * 4 + 3] = (p ushr 24).toByte()
        }
        raw.write(filter)
        for (i in 0 until stride) {
            val a = if (i >= 4) row[i - 4].toInt() and 0xff else 0
            val b = previous[i].toInt() and 0xff
            val c = if (i >= 4) previous[i - 4].toInt() and 0xff else 0
            val predictor = when (filter) {
                0 -> 0
                1 -> a
                2 -> b
                3 -> (a + b) / 2
                else -> paeth(a, b, c)
            }
            raw.write((row[i].toInt() - predictor) and 0xff)
        }
        previous = row
    }
    chunk("IDAT", ByteArrayOutputStream().also { DeflaterOutputStream(it).use { d -> d.write(raw.toByteArray()) } }.toByteArray())
    chunk("IEND", ByteArray(0))
    return out.toByteArray()
}

private fun paeth(a: Int, b: Int, c: Int): Int {
    val p = a + b - c
    val pa = kotlin.math.abs(p - a)
    val pb = kotlin.math.abs(p - b)
    val pc = kotlin.math.abs(p - c)
    return if (pa <= pb && pa <= pc) a else if (pb <= pc) b else c
}

/** 8-bit RGB or RGBA, not interlaced. */
private fun readPng(bytes: ByteArray): ImagePipelineTest.Raster {
    val data = DataInputStream(ByteArrayInputStream(bytes, 8, bytes.size - 8))
    var width = 0
    var height = 0
    var channels = 0
    val idat = ByteArrayOutputStream()
    while (true) {
        val length = data.readInt()
        val type = String(ByteArray(4).also { data.readFully(it) }, Charsets.ISO_8859_1)
        val body = ByteArray(length).also { data.readFully(it) }
        data.readInt()
        when (type) {
            "IHDR" -> DataInputStream(ByteArrayInputStream(body)).apply {
                width = readInt(); height = readInt()
                check(readByte().toInt() == 8)
                channels = when (readByte().toInt()) { 2 -> 3; 6 -> 4; else -> error("colour type") }
            }
            "IDAT" -> idat.write(body)
            "IEND" -> break
        }
    }
    val raw = InflaterInputStream(ByteArrayInputStream(idat.toByteArray())).readBytes()
    val stride = width * channels
    val image = ImagePipelineTest.Raster(width, height)
    var previous = ByteArray(stride)
    var offset = 0
    for (y in 0 until height) {
        val filter = raw[offset++].toInt()
        val row = ByteArray(stride)
        for (i in 0 until stride) {
            val a = if (i >= channels) row[i - channels].toInt() and 0xff else 0
            val b = previous[i].toInt() and 0xff
            val c = if (i >= channels) previous[i - channels].toInt() and 0xff else 0
            val predictor = when (filter) {
                0 -> 0
                1 -> a
                2 -> b
                3 -> (a + b) / 2
                4 -> paeth(a, b, c)
                else -> error("filter")
            }
            row[i] = ((raw[offset++].toInt() + predictor) and 0xff).toByte()
        }
        for (x in 0 until width) {
            val r = row[x * channels].toInt() and 0xff
            val g = row[x * channels + 1].toInt() and 0xff
            val bl = row[x * channels + 2].toInt() and 0xff
            val alpha = if (channels == 4) row[x * channels + 3].toInt() and 0xff else 0xff
            image.argb[y * width + x] = (alpha shl 24) or (r shl 16) or (g shl 8) or bl
        }
        previous = row
    }
    return image
}

/** JPEG's container (SOI, an optional APP1 [exif] segment, EOI) around the test's raw pixels (a COM segment's size, then ARGB). */
private fun writeJpeg(image: ImagePipelineTest.Raster, exif: String?): ByteArray {
    val out = ByteArrayOutputStream()
    val data = DataOutputStream(out)
    data.write(byteArrayOf(0xff.toByte(), 0xd8.toByte()))
    if (exif != null) {
        val payload = exif.toByteArray(Charsets.ISO_8859_1)
        data.write(byteArrayOf(0xff.toByte(), 0xe1.toByte()))
        data.writeShort(payload.size + 2)
        data.write(payload)
    }
    data.write(byteArrayOf(0xff.toByte(), 0xfe.toByte()))
    data.writeShort(10)
    data.writeInt(image.width)
    data.writeInt(image.height)
    image.argb.forEach { data.writeInt(it or (0xff shl 24)) }
    data.write(byteArrayOf(0xff.toByte(), 0xd9.toByte()))
    return out.toByteArray()
}

private fun readJpeg(bytes: ByteArray): ImagePipelineTest.Raster {
    val data = DataInputStream(ByteArrayInputStream(bytes, 2, bytes.size - 2))
    while (true) {
        check(data.readUnsignedByte() == 0xff)
        val marker = data.readUnsignedByte()
        val length = data.readUnsignedShort()
        if (marker != 0xfe) {
            data.skipBytes(length - 2)
            continue
        }
        val image = ImagePipelineTest.Raster(data.readInt(), data.readInt())
        for (i in image.argb.indices) image.argb[i] = data.readInt()
        return image
    }
}
