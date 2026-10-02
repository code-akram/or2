package io.github.code_akram.or2.paste

import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.IOException
import java.io.InputStream
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import kotlin.time.Duration.Companion.milliseconds
import kotlin.time.Duration.Companion.seconds

/**
 * Reading an image from another app's provider (contracts.md, "Image paste"): only `content:`, never
 * stuck on a provider that does not answer, and a keyboard's grant given back exactly once on every path.
 */
class ImageSourceTest {
    /** A provider that blocks its open, or its first read, until it is aborted, as a hostile one may. */
    private class Stuck(private val inOpen: Boolean) : ImageSource {
        val aborted = CountDownLatch(1)
        val aborts = AtomicInteger()
        val opened = CountDownLatch(1)
        val blocked = CountDownLatch(1)

        override fun open(): InputStream {
            opened.countDown()
            if (inOpen) {
                blocked.countDown()
                aborted.await()
                throw IOException("cancelled")
            }
            return object : InputStream() {
                override fun read(): Int {
                    blocked.countDown()
                    aborted.await()
                    throw IOException("closed")
                }
            }
        }

        override fun abort() {
            aborts.incrementAndGet()
            aborted.countDown()
        }
    }

    private class Bytes(private val bytes: ByteArray) : ImageSource {
        var opens = 0
        override fun open(): InputStream = ByteArrayInputStream(bytes).also { opens++ }
        override fun abort() = fail("a finished read is not aborted")
    }

    /** A codec that hands the bytes back, so a preparation's result shows what was read. */
    private object Passthrough : ImageCodec<ByteArray> {
        override fun decode(bytes: ByteArray, target: (width: Int, height: Int) -> PixelSize) = Decoded(bytes, "image/png")
        override fun hasTransparency(image: ByteArray) = false
        override fun encode(image: ByteArray, format: ImageFormat, quality: Int) = image
    }

    @Test
    fun onlyContentUrisAreRead() {
        assertTrue(readableImageScheme("content"))
        for (scheme in listOf("file", null, "", "Content", "http", "android.resource")) {
            assertFalse("$scheme", readableImageScheme(scheme))
        }
        // A share of a `file:` URI, or of one without a scheme, is refused before anything is opened.
        for (scheme in listOf("file", null)) {
            var created = false
            val released = AtomicInteger()
            val prepare = imagePreparation(scheme, { created = true; Bytes(byteArrayOf(1)) }, { released.incrementAndGet() }, Passthrough)
            val refused = runCatching { runBlocking { prepare() } }.exceptionOrNull()
            assertEquals(UNREADABLE, (refused as ImageRefused).message)
            assertFalse(created)
            assertEquals(1, released.get())
        }
    }

    @Test
    fun aReadImageIsPreparedAndItsGrantGivenBackOnce() = runBlocking {
        val released = AtomicInteger()
        val source = Bytes(byteArrayOf(1, 2, 3))
        val prepare = imagePreparation("content", { source }, { released.incrementAndGet() }, Passthrough)
        assertArrayEquals(byteArrayOf(1, 2, 3), prepare().bytes)
        // Run again (it never is), still released once.
        prepare()
        assertEquals(1, released.get())
    }

    @Test
    fun aCancelStopsAProviderThatNeverOpensOrNeverReadsAndGivesTheGrantBackAtOnce() = runBlocking {
        for (inOpen in listOf(true, false)) {
            val source = Stuck(inOpen)
            val released = AtomicInteger()
            val prepare = imagePreparation("content", { source }, { released.incrementAndGet() }, Passthrough)
            val job = async(Dispatchers.Default, start = CoroutineStart.UNDISPATCHED) { prepare() }
            assertTrue("blocked", source.blocked.await(5, TimeUnit.SECONDS))
            assertEquals(0, released.get())
            job.cancel()
            // The caller is free at once, although the provider has not answered on its own.
            withTimeout(2.seconds) { job.join() }
            assertTrue(job.isCancelled)
            assertEquals("inOpen=$inOpen", 1, source.aborts.get())
            assertEquals("inOpen=$inOpen", 1, released.get())
        }
    }

    @Test
    fun aProviderTooSlowToAnswerIsRefusedAtTheDeadlineAndAborted() = runBlocking {
        for (inOpen in listOf(true, false)) {
            val source = Stuck(inOpen)
            val released = AtomicInteger()
            val prepare = imagePreparation("content", { source }, { released.incrementAndGet() }, Passthrough, timeout = 200.milliseconds)
            val refused = runCatching { withTimeout(5.seconds) { prepare() } }.exceptionOrNull()
            assertEquals("inOpen=$inOpen", TOO_SLOW, (refused as ImageRefused).message)
            assertEquals(1, source.aborts.get())
            assertEquals(1, released.get())
        }
    }

    @Test
    fun aProviderThatFailsIsUnreadableAndAnOverlongOneTooLarge() = runBlocking {
        val failing = object : ImageSource {
            override fun open(): InputStream = throw SecurityException("no grant")
            override fun abort() {}
        }
        assertEquals(UNREADABLE, runCatching { readImage(failing) }.exceptionOrNull()?.message)
        val endless = object : ImageSource {
            override fun open(): InputStream = object : InputStream() {
                override fun read() = 7
                override fun read(b: ByteArray, off: Int, len: Int): Int = len.also { b.fill(7, off, off + len) }
            }
            override fun abort() {}
        }
        assertEquals(TOO_LARGE, runCatching { readImage(endless) }.exceptionOrNull()?.message)
    }

    @Test
    fun releaseRunsOnce() {
        val count = AtomicInteger()
        val once = Once { count.incrementAndGet() }
        repeat(3) { once() }
        assertEquals(1, count.get())
    }
}
