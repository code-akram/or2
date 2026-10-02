package io.github.code_akram.or2.paste

import io.github.code_akram.or2.ffi.HostException
import io.github.code_akram.or2.ui.NoticeTone
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.withTimeoutOrNull
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

/** Quoting and the insert target, and one terminal's upload ([ImagePaste]) from start to its notice strip. */
@OptIn(ExperimentalCoroutinesApi::class)
class ImagePasteTest {
    @Test
    fun aPathIsQuotedOnlyWhenItNeedsItAndInsertedAfterASpaceWithoutEnter() {
        val plain = "/home/dev/.cache/or2/images/or2-20261002-153012-a1b2c3.png"
        assertEquals(plain, shellQuote(plain))
        assertEquals(" $plain", pathInsertion(plain))
        assertEquals("'/home/my user/.cache/or2/images/a.png'", shellQuote("/home/my user/.cache/or2/images/a.png"))
        assertEquals("'/home/o'\\''brien/a.png'", shellQuote("/home/o'brien/a.png"))
        for (special in listOf("/h/\$x.png", "/h/a*b.png", "/h/~a.png", "/h/a;b", "/h/a\"b", "/h/a\\b", "/h/é.png")) {
            val quoted = shellQuote(special)
            assertTrue(quoted, quoted.startsWith("'") && quoted.endsWith("'"))
        }
        assertEquals("''", shellQuote(""))
        for (path in listOf(plain, "/home/my user/a.png")) {
            val inserted = pathInsertion(path)
            assertTrue(inserted.startsWith(" "))
            assertFalse("no Enter", inserted.contains('\n') || inserted.contains('\r'))
        }
    }

    @Test
    fun thePathGoesIntoAnOpenComposerElseIntoTheTerminal() {
        assertEquals(InsertTarget.COMPOSER, insertTarget(composerOpen = true))
        assertEquals(InsertTarget.TERMINAL, insertTarget(composerOpen = false))
        assertEquals("look at /p.png", composerWithPath("look at", "/p.png"))
        assertEquals(" /p.png", composerWithPath("", "/p.png"))
    }

    private class Upload {
        val calls = mutableListOf<Pair<String, Int>>()
        var gate: CompletableDeferred<Unit>? = null
        var failure: Exception? = null
        suspend fun invoke(bytes: ByteArray, extension: String): String {
            calls += extension to bytes.size
            gate?.await()
            failure?.let { throw it }
            return "/home/u/.cache/or2/images/or2-${calls.size}.$extension"
        }
    }

    // Its own scope on the test scheduler: `advanceUntilIdle` runs it (it does not wait for `backgroundScope`).
    private fun TestScope.paste(upload: Upload) =
        ImagePaste(CoroutineScope(StandardTestDispatcher(testScheduler))) { bytes, extension -> upload.invoke(bytes, extension) }

    private val png = PreparedImage(byteArrayOf(1, 2, 3), ImageFormat.PNG)

    @Test
    fun anUploadShowsItsProgressThenDeliversItsPathOnce() = runTest(StandardTestDispatcher()) {
        val upload = Upload().apply { gate = CompletableDeferred() }
        val paste = paste(upload)
        assertEquals(UploadState.Idle, paste.state.value)
        assertTrue(paste.start { png })
        assertEquals(UploadState.Uploading, paste.state.value)
        val notice = uploadNotice(paste.state.value)!!
        assertEquals("Uploading image\u2026", notice.notice.text)
        assertTrue(notice.notice.busy)
        assertEquals("Cancel", notice.action)
        // One at a time: a second image while one uploads is not taken.
        assertFalse(paste.start { png })
        advanceUntilIdle()
        assertEquals(listOf("png" to 3), upload.calls)
        upload.gate!!.complete(Unit)
        advanceUntilIdle()
        assertEquals(UploadState.Idle, paste.state.value)
        assertNull(uploadNotice(paste.state.value))
        assertEquals("/home/u/.cache/or2/images/or2-1.png", paste.paths.first())
        // Delivered once: nothing more is waiting.
        assertNull(withTimeoutOrNull(1_000) { paste.paths.first() })
    }

    @Test
    fun aPathWaitsForTheScreenThatInsertsIt() = runTest(StandardTestDispatcher()) {
        val upload = Upload()
        val paste = paste(upload)
        paste.start { png }
        advanceUntilIdle()
        paste.start { PreparedImage(byteArrayOf(9), ImageFormat.JPEG) }
        advanceUntilIdle()
        // No screen was collecting: both paths are there, in order, when one does.
        val received = mutableListOf<String>()
        val collecting = launch { paste.paths.toList(received) }
        advanceUntilIdle()
        collecting.cancel()
        assertEquals(listOf("/home/u/.cache/or2/images/or2-1.png", "/home/u/.cache/or2/images/or2-2.jpg"), received)
    }

    @Test
    fun cancelStopsTheUploadAndNothingIsInserted() = runTest(StandardTestDispatcher()) {
        val upload = Upload().apply { gate = CompletableDeferred() }
        val paste = paste(upload)
        paste.start { png }
        advanceUntilIdle()
        paste.cancel()
        assertEquals(UploadState.Idle, paste.state.value)
        upload.gate!!.complete(Unit)
        advanceUntilIdle()
        assertEquals(UploadState.Idle, paste.state.value)
        assertNull(withTimeoutOrNull(1_000) { paste.paths.first() })
        // A new upload can start at once.
        upload.gate = null
        assertTrue(paste.start { png })
        advanceUntilIdle()
        assertEquals("/home/u/.cache/or2/images/or2-2.png", paste.paths.first())
    }

    /** Hostile answers a host could give for the image's path: each would act in a terminal, quoted or not. */
    private val hostilePaths = listOf(
        "/home/x\u0003touch /tmp/pwn\n/or2-1.png", // ETX ends the quote's line, LF runs the rest
        "/home/\u001b]0;owned\u0007/or2-1.png", // ESC: a terminal sequence
        "/home/x\rrm -rf ~/or2-1.png", // CR
        "/home/x\n/or2-1.png", // LF
        "/home/x\u001b[201~touch /tmp/pwn\n/or2-1.png", // a bracketed paste's end marker
        "/home/x\u009b2J/or2-1.png", // C1 CSI
        "/home/x\u0000/or2-1.png", // NUL
        "/home/x\u007f/or2-1.png", // DEL
        "relative/or2-1.png",
    )

    @Test
    fun aPathWithControlCharactersIsNeverInsertedInEitherTarget() = runTest(StandardTestDispatcher()) {
        for (path in hostilePaths) {
            assertFalse(path, insertablePath(path))
            // The terminal's insertion (bracketed paste or typed) and the composer's both refuse it.
            assertThrows(IllegalArgumentException::class.java) { pathInsertion(path) }
            assertThrows(IllegalArgumentException::class.java) { composerWithPath("look at", path) }
        }
        assertTrue(insertablePath("/home/zoë's files/or2-1.png"))
        // An upload whose host answers one fails in words, and nothing reaches the screen.
        for (path in hostilePaths) {
            val paste = ImagePaste(CoroutineScope(StandardTestDispatcher(testScheduler))) { _, _ -> path }
            assertTrue(paste.start { png })
            advanceUntilIdle()
            assertEquals(UploadState.Failed(UNUSABLE_PATH), paste.state.value)
            assertNull(withTimeoutOrNull(1_000) { paste.paths.first() })
        }
    }

    @Test
    fun aSecondImageWhileOneUploadsIsRefusedInWords() = runTest(StandardTestDispatcher()) {
        val upload = Upload().apply { gate = CompletableDeferred() }
        val paste = paste(upload)
        assertTrue(paste.start { png })
        advanceUntilIdle()
        var second = false
        assertFalse(paste.start { second = true; png })
        // The strip says so, still with Cancel for the running upload.
        assertEquals(UploadState.AlreadyUploading, paste.state.value)
        assertTrue(paste.state.value.uploading)
        val notice = uploadNotice(paste.state.value)!!
        assertEquals(ALREADY_UPLOADING, notice.notice.text)
        assertTrue(notice.notice.busy)
        assertEquals("Cancel", notice.action)
        // For a moment, then back to the upload's own words.
        advanceTimeBy(ALREADY_SHOWN + 1)
        assertEquals(UploadState.Uploading, paste.state.value)
        // Its end speaks for itself, even over the moment.
        assertFalse(paste.start { png })
        upload.gate!!.complete(Unit)
        advanceUntilIdle()
        assertEquals(UploadState.Idle, paste.state.value)
        assertFalse("the second image was never read", second)
        assertEquals(1, upload.calls.size)
    }

    @Test
    fun aTakenImageIsAlwaysPreparedSoItsGrantIsGivenBackEvenWhenCancelledAtOnce() = runTest(StandardTestDispatcher()) {
        val upload = Upload()
        val paste = paste(upload)
        var prepared = 0
        assertTrue(paste.start { prepared++; png })
        // Cancelled before the upload's coroutine was ever dispatched.
        paste.cancel()
        advanceUntilIdle()
        assertEquals(1, prepared)
        assertEquals(UploadState.Idle, paste.state.value)
        assertNull(withTimeoutOrNull(1_000) { paste.paths.first() })
    }

    @Test
    fun aFailureShowsItsReasonUntilDismissed() = runTest(StandardTestDispatcher()) {
        val upload = Upload().apply { failure = HostException.SftpUnavailable() }
        val paste = paste(upload)
        paste.start { png }
        advanceUntilIdle()
        assertEquals(UploadState.Failed("SFTP is not available on this host"), paste.state.value)
        val notice = uploadNotice(paste.state.value)!!
        assertEquals(NoticeTone.Warning, notice.notice.tone)
        assertFalse(notice.notice.busy)
        assertEquals("Dismiss", notice.action)
        paste.dismiss()
        assertEquals(UploadState.Idle, paste.state.value)
        // A refused image never reaches the host.
        paste.start { throw ImageRefused(TOO_LARGE) }
        advanceUntilIdle()
        assertEquals(UploadState.Failed(TOO_LARGE), paste.state.value)
        assertEquals(1, upload.calls.size)
        // A new upload replaces a failure.
        upload.failure = null
        paste.start { png }
        assertEquals(UploadState.Uploading, paste.state.value)
        advanceUntilIdle()
        assertEquals(UploadState.Idle, paste.state.value)
    }

    @Test
    fun failuresAreReasonsWithoutPaths() {
        assertEquals("SFTP is not available on this host", uploadErrorMessage(HostException.SftpUnavailable()))
        assertEquals(TOO_LARGE, uploadErrorMessage(HostException.TooLarge()))
        assertEquals("Not sent: the host is not connected", uploadErrorMessage(HostException.NotConnected()))
        assertEquals("Not sent: the host is not connected", uploadErrorMessage(HostException.Closed()))
        assertEquals("Upload failed: creating the image: permission denied",
            uploadErrorMessage(HostException.CommandFailed("creating the image: permission denied")))
        assertEquals(UNREADABLE, uploadErrorMessage(ImageRefused(UNREADABLE)))
        assertEquals("Upload failed", uploadErrorMessage(IllegalStateException("/home/secret")))
    }
}
