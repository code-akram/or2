package io.github.code_akram.or2.terminal

/**
 * Clipboard writes from hosts (OSC 52, OSC 1337 Copy; `on_clipboard_write`) on their way to the Android
 * clipboard. Nothing is ever read back: the core never answers a read request.
 *
 * - Off when [enabled] says so (Settings, "Copy from the host", on by default), checked at each write.
 * - Text over [MAX_BYTES] (UTF-8) is dropped whole; empty text is nothing to copy.
 * - At most one write per [INTERVAL_MS] per terminal: the first goes at once, and later ones in the
 *   window replace a single pending write, made when the window ends. A program that copies in a loop
 *   cannot flood the clipboard.
 *
 * Called on one thread (main); [schedule] runs its action there too. Android 13+ shows its own copy
 * confirmation, so this adds none.
 */
class HostClipboard(
    private val enabled: () -> Boolean,
    private val now: () -> Long,
    private val schedule: (delayMs: Long, action: () -> Unit) -> Unit,
    private val write: (String) -> Unit,
) {
    private class Window(var last: Long) {
        var pending: String? = null
        var scheduled = false
    }

    private val windows = HashMap<Long, Window>()

    fun offer(terminalId: Long, text: String) {
        if (text.isEmpty() || utf8Length(text) > MAX_BYTES || !enabled()) return
        val time = now()
        // Windows that are over and have nothing pending are forgotten.
        windows.values.removeAll { !it.scheduled && time - it.last >= INTERVAL_MS }
        val window = windows[terminalId]
        if (window == null) {
            windows[terminalId] = Window(time)
            write(text)
            return
        }
        window.pending = text
        if (!window.scheduled) {
            window.scheduled = true
            schedule(window.last + INTERVAL_MS - time) { flush(window) }
        }
    }

    private fun flush(window: Window) {
        window.scheduled = false
        val text = window.pending ?: return
        window.pending = null
        window.last = now()
        if (enabled()) write(text)
    }

    companion object {
        const val MAX_BYTES = 1 shl 20
        const val INTERVAL_MS = 500L

        /** The UTF-8 length of [text] without encoding it (an unpaired surrogate counts as `?`). */
        fun utf8Length(text: String): Int {
            var bytes = 0
            var index = 0
            while (index < text.length) {
                val char = text[index]
                bytes += when {
                    char.code < 0x80 -> 1
                    char.code < 0x800 -> 2
                    char.isHighSurrogate() && index + 1 < text.length && text[index + 1].isLowSurrogate() -> {
                        index++
                        4
                    }
                    char.isSurrogate() -> 1
                    else -> 3
                }
                index++
            }
            return bytes
        }
    }
}
