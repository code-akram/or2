package io.github.code_akram.or2.app

import android.view.WindowManager
import androidx.activity.compose.LocalActivity
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect

/**
 * Settings' "Keep screen on": while this is composed with [on] true (the terminal destination calls it), the window's
 * keep-screen-on flag is set, and it is cleared the moment the terminal leaves the screen or the setting goes off. A
 * window that is not visible does not hold the screen on, so leaving the app needs nothing more.
 */
@Composable
fun KeepScreenOn(on: Boolean) {
    val window = LocalActivity.current?.window ?: return
    DisposableEffect(window, on) {
        if (on) window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        onDispose { if (on) window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON) }
    }
}
