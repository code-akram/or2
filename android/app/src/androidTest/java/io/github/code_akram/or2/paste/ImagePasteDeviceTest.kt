package io.github.code_akram.or2.paste

import android.content.ClipDescription
import android.content.Intent
import android.net.Uri
import android.provider.MediaStore
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputContentInfo
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.github.code_akram.or2.MainActivity
import io.github.code_akram.or2.terminal.IMAGE_PICKER
import io.github.code_akram.or2.terminal.TerminalView
import io.github.code_akram.or2.terminal.imagePickRequest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

/** Image paste's platform edges (contracts.md, "Image paste"): the share target, the picker and a keyboard's images. */
@RunWith(AndroidJUnit4::class)
class ImagePasteDeviceTest {
    private val instrumentation get() = InstrumentationRegistry.getInstrumentation()
    private val context get() = instrumentation.targetContext

    @Test
    fun or2TakesImagesSharedFromOtherAppsAndNothingElse() {
        val pm = context.packageManager
        fun targets(action: String, type: String) = pm.queryIntentActivities(Intent(action).setType(type).setPackage(context.packageName), 0)
            .map { it.activityInfo.name }
        for (action in listOf(Intent.ACTION_SEND, Intent.ACTION_SEND_MULTIPLE)) {
            for (type in listOf("image/png", "image/jpeg", "image/webp", "image/gif", "image/*")) {
                assertEquals("$action $type", listOf(MainActivity::class.java.name), targets(action, type))
            }
            assertTrue(targets(action, "text/plain").isEmpty())
            assertTrue(targets(action, "application/pdf").isEmpty())
        }
        // The intent it parses: a stream of an image type, nothing else.
        val uri = Uri.parse("content://io.github.code_akram.or2.test/image.png")
        assertEquals(Share(listOf(uri)), sharedImages(Intent(Intent.ACTION_SEND).setType("image/png").putExtra(Intent.EXTRA_STREAM, uri)))
        assertNull(sharedImages(Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_STREAM, uri)))
        assertNull(sharedImages(Intent(Intent.ACTION_VIEW).setType("image/png").putExtra(Intent.EXTRA_STREAM, uri)))
        assertNull(sharedImages(null))
        // Only content: a file of or2's own, or a URI without a scheme, is never taken, but counted.
        val others = listOf("file:///data/data/io.github.code_akram.or2/files/secret.png", "/sdcard/x.png", "x.png").map(Uri::parse)
        for (other in others) {
            val share = sharedImages(Intent(Intent.ACTION_SEND).setType("image/png").putExtra(Intent.EXTRA_STREAM, other))
            assertEquals("$other", Share<Uri>(emptyList(), refused = 1), share)
        }
        // Several: each item checked, the content: ones kept in order.
        val several = listOf(uri, others[0], Uri.parse("content://io.github.code_akram.or2.test/b.jpg"))
        val multiple = Intent(Intent.ACTION_SEND_MULTIPLE).setType("image/*").putParcelableArrayListExtra(Intent.EXTRA_STREAM, ArrayList(several))
        assertEquals(Share(listOf(several[0], several[2]), refused = 1), sharedImages(multiple))
        val shares = ImageShares()
        shares.offer(Share(listOf(uri)))
        assertEquals(Share(listOf(uri)), shares.take())
        assertNull(shares.take())
    }

    @Test
    fun theAttachButtonsPickerTakesSeveralImagesUpToTheQueuesCap() {
        val intent = IMAGE_PICKER.createIntent(context, imagePickRequest())
        if (intent.action == MediaStore.ACTION_PICK_IMAGES) {
            assertEquals("image/*", intent.type)
            assertEquals(MAX_IMAGES, intent.getIntExtra(MediaStore.EXTRA_PICK_IMAGES_MAX, 0))
        } else {
            // A device without the system picker: the documents fallback, several allowed (the queue caps them).
            assertTrue(intent.getBooleanExtra(Intent.EXTRA_ALLOW_MULTIPLE, false))
        }
    }

    @Test
    fun theTerminalTakesAKeyboardsImageOnlyWhileItHasAnUpload() {
        instrumentation.runOnMainSync {
            val view = TerminalView(context)
            val attrs = EditorInfo()
            view.onCreateInputConnection(attrs)
            assertTrue("no upload: no images offered", attrs.contentMimeTypes.isNullOrEmpty())

            val taken = mutableListOf<Uri>()
            var released = 0
            view.onImage = { uri, release -> taken += uri; release(); released++; true }
            val withImages = EditorInfo()
            val connection = view.onCreateInputConnection(withImages)
            assertEquals(listOf("image/*"), withImages.contentMimeTypes?.toList())

            val uri = Uri.parse("content://io.github.code_akram.or2.test/clip.png")
            val image = InputContentInfo(uri, ClipDescription("screenshot", arrayOf("image/png")), null)
            assertTrue(connection.commitContent(image, 0, null))
            assertEquals(listOf(uri), taken)
            assertEquals(1, released)
            // Anything but an image is refused.
            val text = InputContentInfo(uri, ClipDescription("text", arrayOf("text/plain")), null)
            assertFalse(connection.commitContent(text, 0, null))
            assertEquals(1, taken.size)
            // Without an upload, none.
            view.onImage = null
            assertFalse(view.onCreateInputConnection(EditorInfo()).commitContent(image, 0, null))
        }
    }
}
