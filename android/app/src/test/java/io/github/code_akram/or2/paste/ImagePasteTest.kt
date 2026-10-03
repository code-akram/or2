package io.github.code_akram.or2.paste

import io.github.code_akram.or2.ffi.HostException
import io.github.code_akram.or2.ui.NoticeTone
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

/** Quoting and the insert target, and one terminal's upload queue ([ImagePaste]) from start to its notice strip. */
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
        assertEquals("look at /p.png", composerWithPaths("look at", listOf("/p.png")))
        assertEquals(" /p.png", composerWithPaths("", listOf("/p.png")))
    }

    private class Upload {
        val calls = mutableListOf<Pair<String, Int>>()
        var gate: CompletableDeferred<Unit>? = null
        var failure: Exception? = null

        /** The calls (counted from 1) that fail ([failing]) or answer a path that is no path to insert ([unusable]). */
        var failing = emptySet<Int>()
        var unusable = emptySet<Int>()
        suspend fun invoke(bytes: ByteArray, extension: String): String {
            calls += extension to bytes.size
            val call = calls.size
            gate?.await()
            failure?.let { throw it }
            if (call in failing) throw HostException.SftpUnavailable()
            if (call in unusable) return "/home/u/x\u001b[201~/or2-$call.$extension"
            return "/home/u/.cache/or2/images/or2-$call.$extension"
        }
    }

    /** The path [Upload] answers for its [call]th image. */
    private fun path(call: Int, extension: String = "png") = "/home/u/.cache/or2/images/or2-$call.$extension"

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
        assertEquals(UploadState.Uploading(), paste.state.value)
        val notice = uploadNotice(paste.state.value)!!
        assertEquals("Uploading image\u2026", notice.text)
        assertTrue(notice.busy)
        assertEquals("Cancel", notice.action)
        advanceUntilIdle()
        assertEquals(listOf("png" to 3), upload.calls)
        upload.gate!!.complete(Unit)
        advanceUntilIdle()
        assertEquals(UploadState.Idle, paste.state.value)
        assertNull(uploadNotice(paste.state.value))
        assertEquals(listOf("/home/u/.cache/or2/images/or2-1.png"), paste.paths.first())
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
        val received = mutableListOf<List<String>>()
        val collecting = launch { paste.paths.toList(received) }
        advanceUntilIdle()
        collecting.cancel()
        assertEquals(listOf(listOf("/home/u/.cache/or2/images/or2-1.png"), listOf("/home/u/.cache/or2/images/or2-2.jpg")), received)
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
        assertEquals(listOf("/home/u/.cache/or2/images/or2-2.png"), paste.paths.first())
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
            assertThrows(IllegalArgumentException::class.java) { composerWithPaths("look at", listOf(path)) }
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

    /** An image of [size] bytes, so the upload's calls show which image went when. */
    private fun image(size: Int, format: ImageFormat = ImageFormat.PNG) = PreparedImage(ByteArray(size), format)

    /** Lets the upload running at [upload]'s gate end, and holds the next one at a new gate. */
    private fun TestScope.finishOne(upload: Upload) {
        val gate = upload.gate!!
        upload.gate = CompletableDeferred()
        gate.complete(Unit)
        advanceUntilIdle()
    }

    @Test
    fun threeImagesUploadOneAtATimeInOrderAndAreInsertedOnceTogether() = runTest(StandardTestDispatcher()) {
        val upload = Upload().apply { gate = CompletableDeferred() }
        val paste = paste(upload)
        assertTrue(paste.start { image(1) })
        assertTrue(paste.start { image(2, ImageFormat.JPEG) })
        assertTrue(paste.start { image(3) })
        advanceUntilIdle()
        // One at a time: the second waits for the first.
        assertEquals(listOf("png" to 1), upload.calls)
        assertEquals(UploadState.Uploading(1, 3), paste.state.value)
        finishOne(upload)
        assertEquals(listOf("png" to 1, "jpg" to 2), upload.calls)
        assertEquals(UploadState.Uploading(2, 3), paste.state.value)
        // Nothing is inserted while the queue runs.
        assertNull(withTimeoutOrNull(1_000) { paste.paths.first() })
        upload.gate!!.complete(Unit)
        advanceUntilIdle()
        assertEquals(listOf("png" to 1, "jpg" to 2, "png" to 3), upload.calls)
        assertEquals(UploadState.Idle, paste.state.value)
        val paths = paste.paths.first()
        assertEquals(listOf(path(1), path(2, "jpg"), path(3)), paths)
        assertEquals(" ${path(1)} ${path(2, "jpg")} ${path(3)}", pathsInsertion(paths))
        // Delivered once.
        assertNull(withTimeoutOrNull(1_000) { paste.paths.first() })
    }

    @Test
    fun anImageJoiningARunningQueueIsUploadedAndInsertedWithTheRest() = runTest(StandardTestDispatcher()) {
        val upload = Upload().apply { gate = CompletableDeferred() }
        val paste = paste(upload)
        assertTrue(paste.start { image(1) })
        advanceUntilIdle()
        assertEquals(UploadState.Uploading(1, 1), paste.state.value)
        // A keyboard's image while the picker's uploads: it joins, and the count grows.
        assertTrue(paste.start { image(2) })
        assertEquals(UploadState.Uploading(1, 2), paste.state.value)
        upload.gate!!.complete(Unit)
        advanceUntilIdle()
        assertEquals(listOf(path(1), path(2)), paste.paths.first())
        assertEquals(UploadState.Idle, paste.state.value)
    }

    @Test
    fun theEleventhPendingImageIsRefusedInWordsAndItsSourceIsTold() = runTest(StandardTestDispatcher()) {
        val upload = Upload().apply { gate = CompletableDeferred() }
        val paste = paste(upload)
        repeat(MAX_IMAGES) { assertTrue(paste.start { image(it + 1) }) }
        advanceUntilIdle()
        var eleventh = false
        assertFalse(paste.start { eleventh = true; png })
        // The strip says so, still with Cancel for the running queue.
        val full = paste.state.value
        assertEquals(UploadState.Uploading(1, MAX_IMAGES, full = true), full)
        assertTrue(full.uploading)
        val notice = uploadNotice(full)!!
        assertEquals(TOO_MANY_IMAGES, notice.text)
        assertEquals("At most 10 images at a time", notice.text)
        assertTrue(notice.busy)
        assertEquals("Cancel", notice.action)
        // For a moment, then back to the queue's own words.
        advanceTimeBy(ALREADY_SHOWN + 1)
        assertEquals(UploadState.Uploading(1, MAX_IMAGES), paste.state.value)
        assertFalse("the refused image was never read", eleventh)
        // Pending or running counts: once one is done, there is room again.
        finishOne(upload)
        assertEquals(UploadState.Uploading(2, MAX_IMAGES), paste.state.value)
        assertTrue(paste.start { image(11) })
        assertEquals(UploadState.Uploading(2, MAX_IMAGES + 1), paste.state.value)
        assertFalse(paste.start { png })
        upload.gate!!.complete(Unit)
        advanceUntilIdle()
        assertEquals(UploadState.Idle, paste.state.value)
        assertEquals((1..11).map { "png" to it }, upload.calls)
        assertEquals((1..11).map { path(it) }, paste.paths.first())
    }

    @Test
    fun aFailureInTheMiddleSkipsThatImageAndTheOthersStillUploadAndAreInserted() = runTest(StandardTestDispatcher()) {
        val upload = Upload().apply { failing = setOf(2) }
        val paste = paste(upload)
        for (size in 1..3) assertTrue(paste.start { image(size) })
        advanceUntilIdle()
        assertEquals(3, upload.calls.size)
        assertEquals(listOf(path(1), path(3)), paste.paths.first())
        assertEquals(UploadState.Failed("1 of 3 images failed: SFTP is not available on this host"), paste.state.value)
        val notice = uploadNotice(paste.state.value)!!
        assertEquals(NoticeTone.Warning, notice.tone)
        assertEquals("Dismiss", notice.action)
        paste.dismiss()
        assertEquals(UploadState.Idle, paste.state.value)
        // A refused image and an unusable answer count as failed too, and the first reason is shown.
        upload.failing = emptySet()
        upload.unusable = setOf(5)
        paste.start { throw ImageRefused(TOO_LARGE) }
        paste.start { image(4) }
        paste.start { image(5) }
        advanceUntilIdle()
        assertEquals(listOf(path(4)), paste.paths.first())
        assertEquals(UploadState.Failed("2 of 3 images failed: $TOO_LARGE"), paste.state.value)
        // All failed: nothing is inserted.
        paste.start { throw ImageRefused(UNREADABLE) }
        paste.start { throw ImageRefused(TOO_LARGE) }
        advanceUntilIdle()
        assertEquals(UploadState.Failed("2 of 2 images failed: $UNREADABLE"), paste.state.value)
        assertNull(withTimeoutOrNull(1_000) { paste.paths.first() })
    }

    @Test
    fun cancelInsertsNothingAndEveryQueuedImageIsStillPreparedWithoutUploading() = runTest(StandardTestDispatcher()) {
        val upload = Upload().apply { gate = CompletableDeferred() }
        val paste = paste(upload)
        var prepared = 0
        val cancelledWhenPrepared = mutableListOf<Boolean>()
        val counted: suspend () -> PreparedImage = {
            prepared++
            cancelledWhenPrepared += !currentCoroutineContext().isActive
            png
        }
        repeat(3) { assertTrue(paste.start(counted)) }
        advanceUntilIdle()
        assertEquals(1, prepared)
        paste.cancel()
        assertEquals(UploadState.Idle, paste.state.value)
        advanceUntilIdle()
        // Every taken image was prepared (so each gave back its grant); the two still queued in a cancelled run.
        assertEquals(3, prepared)
        assertEquals(listOf(false, true, true), cancelledWhenPrepared)
        assertEquals(1, upload.calls.size)
        upload.gate!!.complete(Unit)
        advanceUntilIdle()
        assertEquals(UploadState.Idle, paste.state.value)
        assertNull(withTimeoutOrNull(1_000) { paste.paths.first() })
        // Cancelled before the queue's coroutine ever ran: all prepared, none uploaded.
        upload.gate = null
        repeat(3) { paste.start(counted) }
        paste.cancel()
        advanceUntilIdle()
        assertEquals(6, prepared)
        assertEquals(1, upload.calls.size)
        assertNull(withTimeoutOrNull(1_000) { paste.paths.first() })
        // The next queue starts afresh.
        assertTrue(paste.start { png })
        assertEquals(UploadState.Uploading(1, 1), paste.state.value)
        advanceUntilIdle()
        assertEquals(listOf(path(2)), paste.paths.first())
    }

    @Test
    fun aCancelledRunsLateEndNeverSpeaksForTheNextRun() = runTest(StandardTestDispatcher()) {
        // An upload that ignores its cancel and answers late, as a stuck call might.
        val late = CompletableDeferred<Unit>()
        var calls = 0
        val paste = ImagePaste(CoroutineScope(StandardTestDispatcher(testScheduler))) { _, extension ->
            calls++
            if (calls == 1) withContext(NonCancellable) { late.await() }
            "/home/u/.cache/or2/images/or2-$calls.$extension"
        }
        paste.start { png }
        advanceUntilIdle()
        paste.cancel()
        val gate = CompletableDeferred<Unit>()
        paste.start { gate.await(); png }
        advanceUntilIdle()
        assertEquals(UploadState.Uploading(1, 1), paste.state.value)
        late.complete(Unit)
        advanceUntilIdle()
        // The old run's end changed nothing: the new one still uploads, and only its path arrives.
        assertEquals(UploadState.Uploading(1, 1), paste.state.value)
        gate.complete(Unit)
        advanceUntilIdle()
        assertEquals(UploadState.Idle, paste.state.value)
        assertEquals(listOf("/home/u/.cache/or2/images/or2-2.png"), paste.paths.first())
        assertNull(withTimeoutOrNull(1_000) { paste.paths.first() })
    }

    @Test
    fun theStripCountsTheImagesOfARun() {
        assertEquals("Uploading image…", uploadNotice(UploadState.Uploading())!!.text)
        assertEquals("Uploading image…", uploadNotice(UploadState.Uploading(1, 1))!!.text)
        assertEquals("Uploading image 1 of 3…", uploadNotice(UploadState.Uploading(1, 3))!!.text)
        assertEquals("Uploading image 3 of 4…", uploadNotice(UploadState.Uploading(3, 4))!!.text)
        for (state in listOf(UploadState.Uploading(2, 3), UploadState.Uploading(2, 10, full = true))) {
            val notice = uploadNotice(state)!!
            assertTrue(notice.busy)
            assertEquals(NoticeTone.Info, notice.tone)
            assertEquals("Cancel", notice.action)
        }
        assertNull(queueFailure(emptyList(), 3))
        assertEquals(TOO_LARGE, queueFailure(listOf(TOO_LARGE), 1))
        assertEquals("1 of 2 images failed: $TOO_LARGE", queueFailure(listOf(TOO_LARGE), 2))
        assertEquals("2 of 5 images failed: $UNREADABLE", queueFailure(listOf(UNREADABLE, TOO_LARGE), 5))
    }

    @Test
    fun severalPathsAreInsertedAsOneInOrder() {
        val spaced = "/home/my user/.cache/or2/images/or2-2.jpg"
        assertEquals(" /a.png '$spaced'", pathsInsertion(listOf("/a.png", spaced)))
        assertEquals(pathInsertion("/a.png"), pathsInsertion(listOf("/a.png")))
        assertEquals("look at /a.png /b.jpg", composerWithPaths("look at", listOf("/a.png", "/b.jpg")))
        assertFalse(pathsInsertion(listOf("/a.png", "/b.png")).contains('\n'))
        for (hostile in hostilePaths) {
            assertThrows(IllegalArgumentException::class.java) { pathsInsertion(listOf("/a.png", hostile)) }
        }
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
        assertEquals(NoticeTone.Warning, notice.tone)
        assertFalse(notice.busy)
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
        assertEquals(UploadState.Uploading(), paste.state.value)
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
