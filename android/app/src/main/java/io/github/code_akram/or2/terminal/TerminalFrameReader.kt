package io.github.code_akram.or2.terminal

import io.github.code_akram.or2.ffi.TerminalFrame
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * Main-thread controller, worker-thread native pull/decode. At most one read and one taken frame;
 * never take a second frame until the first is consumed, so moved-row bases remain ordered.
 * Rust continues coalescing its mailbox while a taken frame waits for the next display tick.
 * Detach pauses new reads but retains an in-flight result (including a final closed-session frame).
 */
internal class TerminalFrameReader(
    private val scope: CoroutineScope,
    private val prepare: () -> (() -> TerminalFrame?)?,
    private val ready: () -> Unit,
    private val gone: () -> Unit,
    private val worker: CoroutineDispatcher = Dispatchers.Default,
    private val nanoTime: () -> Long = System::nanoTime,
) {
    data class Read(val frame: TerminalFrame, val nanos: Long, val requestedAt: Long)
    private var active = false
    private var wanted = false
    private var reading = false
    private var pending: Read? = null

    fun start() {
        active = true
        if (pending != null) ready()
        request() // Also drain a notification that arrived before attachment.
    }

    fun stop() {
        active = false
    }

    fun request() {
        wanted = true
        pump()
    }

    fun take(): Read? {
        val result = pending
        pending = null
        pump()
        return result
    }

    private fun pump() {
        if (!active || reading || pending != null || !wanted) return
        val read = prepare() ?: return
        wanted = false
        reading = true
        val requestedAt = nanoTime()
        scope.launch {
            val result = withContext(worker) {
                val start = nanoTime()
                try {
                    read()?.let { Read(it, nanoTime() - start, requestedAt) } to false
                } catch (_: IllegalStateException) {
                    null to true // A borrowed native object was destroyed, as with the old UI pull.
                }
            }
            reading = false
            if (result.second) {
                wanted = false
                gone()
            } else {
                pending = result.first
                if (active && pending != null) ready()
                pump() // A notification during an empty read must not be lost.
            }
        }
    }
}
