package io.github.code_akram.or2.ui

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent

/*
 * The one way the app's screens use the clipboard and the share sheet: the terminal (Copy, Paste), Keys and pairing.
 * Android 13 and later (or2's minimum is 14) show their own confirmation of a copy, so nothing here adds one.
 */

/** Puts [text] on the clipboard, named [label] (what the system's copy confirmation may show). */
fun copyText(context: Context, label: String, text: String) {
    context.getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText(label, text))
}

/**
 * The clipboard's first item as text, or empty: nothing on it, an empty clip, or an item with no text form. A URI or
 * an intent is coerced to text, as the platform's own paste does.
 */
fun clipboardText(context: Context): String {
    val clip = context.getSystemService(ClipboardManager::class.java).primaryClip ?: return ""
    if (clip.itemCount == 0) return ""
    return clip.getItemAt(0)?.coerceToText(context)?.toString().orEmpty()
}

/** Offers [text] to another app through the system's share sheet, titled [title]. */
fun shareText(context: Context, title: String, text: String) {
    val send = Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, text)
    context.startActivity(Intent.createChooser(send, title))
}
