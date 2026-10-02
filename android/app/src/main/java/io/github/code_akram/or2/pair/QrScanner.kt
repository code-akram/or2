package io.github.code_akram.or2.pair

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.os.Handler
import android.os.Looper
import android.util.Size
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.ImageProxy
import androidx.camera.core.Preview
import androidx.camera.core.resolutionselector.ResolutionSelector
import androidx.camera.core.resolutionselector.ResolutionStrategy
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleEventEffect
import androidx.lifecycle.compose.LocalLifecycleOwner
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

/** Whether the camera may be used now. */
fun hasCameraPermission(context: Context) =
    ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED

/**
 * The camera permission, asked only when the user chooses to scan: [granted] reflects it, [request] opens the
 * system dialog (once; a denied permission is explained by the caller, who offers pasting instead).
 */
class CameraAccess(val granted: Boolean, val denied: Boolean, val request: () -> Unit)

@Composable
fun rememberCameraAccess(): CameraAccess {
    val context = LocalContext.current
    var granted by remember { mutableStateOf(hasCameraPermission(context)) }
    var denied by rememberSaveable { mutableStateOf(false) }
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { result ->
        granted = result
        denied = !result
    }
    // Back from the system settings, where the user may have allowed it after all.
    LifecycleEventEffect(Lifecycle.Event.ON_RESUME) {
        if (hasCameraPermission(context)) {
            granted = true
            denied = false
        }
    }
    return CameraAccess(granted, denied) { launcher.launch(Manifest.permission.CAMERA) }
}

/**
 * A live camera preview that reads QR codes with ZXing ([QrDecoder]) and reports the first text it finds, once.
 * CameraX is bound to the lifecycle and released with the composition; frames are analysed on a private thread,
 * newest only, and the preview is not recorded or kept. Needs the camera permission (see [rememberCameraAccess]).
 */
@Composable
fun QrScanner(onCode: (String) -> Unit, modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current
    val latest by rememberUpdatedState(onCode)
    // COMPATIBLE draws the preview in a TextureView, inside the view hierarchy: it is clipped like any other
    // content (the card's rounded corners, the scrolling column under the top bar). The default SurfaceView sits
    // in its own layer that Compose's clips do not reach, so a scrolled-away preview could show over the top bar and
    // the status bar.
    val view = remember {
        PreviewView(context).apply {
            scaleType = PreviewView.ScaleType.FILL_CENTER
            implementationMode = PreviewView.ImplementationMode.COMPATIBLE
        }
    }
    DisposableEffect(lifecycleOwner, view) {
        val executor = Executors.newSingleThreadExecutor()
        val main = ContextCompat.getMainExecutor(context)
        // `done` pauses analysis after a code was reported (a code that was refused leaves the scanner on screen,
        // so it listens again after a moment); `disposed` ends it for good.
        val done = AtomicBoolean(false)
        val disposed = AtomicBoolean(false)
        val handler = Handler(Looper.getMainLooper())
        val decoder = QrDecoder()
        var provider: ProcessCameraProvider? = null
        val future = ProcessCameraProvider.getInstance(context)
        future.addListener({
            if (disposed.get()) return@addListener // Left before the camera provider was ready.
            val cameras = future.get()
            provider = cameras
            val preview = Preview.Builder().build().also { it.surfaceProvider = view.surfaceProvider }
            val analysis = ImageAnalysis.Builder()
                // A terminal QR on a monitor needs more than the 640 x 480 default.
                .setResolutionSelector(
                    ResolutionSelector.Builder()
                        .setResolutionStrategy(
                            ResolutionStrategy(Size(1280, 720), ResolutionStrategy.FALLBACK_RULE_CLOSEST_HIGHER_THEN_LOWER),
                        )
                        .build(),
                )
                .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
                .build()
            analysis.setAnalyzer(executor) { image ->
                val text = if (done.get()) null else decodeFrame(decoder, image)
                image.close()
                if (text != null && done.compareAndSet(false, true)) {
                    main.execute { latest(text) }
                    handler.postDelayed({ done.set(disposed.get()) }, RESUME_AFTER_MS)
                }
            }
            runCatching {
                cameras.unbindAll()
                cameras.bindToLifecycle(lifecycleOwner, CameraSelector.DEFAULT_BACK_CAMERA, preview, analysis)
            }
        }, main)
        onDispose {
            disposed.set(true)
            done.set(true)
            handler.removeCallbacksAndMessages(null)
            provider?.unbindAll()
            executor.shutdown()
        }
    }
    AndroidView({ view }, modifier)
}

private const val RESUME_AFTER_MS = 1500L

/** The luminance (Y) plane of [image] decoded; never throws (a frame that cannot be read is no code). */
private fun decodeFrame(decoder: QrDecoder, image: ImageProxy): String? = try {
    val plane = image.planes[0]
    val buffer = plane.buffer
    val bytes = ByteArray(buffer.remaining())
    buffer.get(bytes)
    decoder.decode(bytes, image.width, image.height, plane.rowStride)
} catch (_: Exception) {
    null
}
