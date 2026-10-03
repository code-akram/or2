package io.github.code_akram.or2.connection

/** The one logcat tag every timing marker uses: `adb logcat -s or2.timing`. */
const val TIMING_TAG = "or2.timing"

/**
 * Lightweight timing markers for the critical paths, read from logcat on a debug build (the
 * application passes a sink that writes to `Log` under [TIMING_TAG] only when it is debuggable; a
 * release build passes none, and every call here returns at once). A *span* is one path from its
 * start (`connect host=3`, `tap host=3 pane=w1:p2`); each [mark] logs the event and the
 * milliseconds since the span began, one line each: `connect host=3 connected ms=412`.
 *
 * Spans name hosts by their id and panes by herdr's pane id, never by label, address or user name,
 * and carry no key or output material.
 *
 * The paths and their events:
 * - `connect host=N`: `unlocked` (the biometric is done, ms=0), `authenticating`, `connected`,
 *   `mosh-server` (the program probe answered `mosh_server()`), `capabilities` (the whole probe
 *   answered), `live` (the first herdr view of any watch), `udp-ok` / `udp-blocked` (the connection's
 *   UDP verdict, from the first mosh terminal that connected or failed), or `failed` (closed before
 *   it connected).
 * - `tap host=N pane=P` (an agent tapped in the inbox) and `reopen host=N` (the return to the
 *   foreground): `begin`, `focused` (herdr acknowledged the pane focus), `terminal-connected`,
 *   `frame` (the first frame was drawn). `reuse host=N pane=P` (a terminal that is already open):
 *   `begin`, `focused`, `frame`.
 * - `resume host=N` (the Resume card or an automatic resume): `begin` (the tap, or the launch),
 *   then the connect and the reopen above, ending with `frame`.
 */
class Timing(
    private val emit: ((String) -> Unit)? = null,
    private val now: () -> Long = { System.nanoTime() / 1_000_000 },
) {
    private val lock = Any()
    private val starts = HashMap<String, Long>()
    private val terminalSpans = HashMap<Long, String>()

    /** False when nothing is recorded (a release build): callers need not build strings for it. */
    val enabled get() = emit != null

    /** Starts [span] now and logs [event] at ms=0, replacing a span of the same name. */
    fun begin(span: String, event: String = "begin") {
        val sink = emit ?: return
        synchronized(lock) { starts[span] = now() }
        sink("$span $event ms=0")
    }

    /** Logs [event] with the time since [span] began; nothing for a span that never began (or has ended). */
    fun mark(span: String, event: String) {
        val sink = emit ?: return
        val ms = synchronized(lock) { starts[span]?.let { now() - it } } ?: return
        sink("$span $event ms=$ms")
    }

    /** [mark], and the span is over: a later mark of the same name is dropped. */
    fun end(span: String, event: String) {
        val sink = emit ?: return
        val ms = synchronized(lock) { starts.remove(span)?.let { now() - it } } ?: return
        sink("$span $event ms=$ms")
    }

    /** Whether [span] is running. */
    fun isRunning(span: String): Boolean = synchronized(lock) { span in starts }

    /** The terminal [id] reports its connect and its first frame to [span]. */
    fun watchTerminal(id: Long, span: String) {
        if (!enabled) return
        synchronized(lock) { terminalSpans[id] = span }
    }

    /** A watched terminal reached `Connected`. */
    fun terminalConnected(id: Long) {
        val span = synchronized(lock) { terminalSpans[id] } ?: return
        mark(span, "terminal-connected")
    }

    /**
     * A terminal drew a frame (the view reports every one): for a watched terminal that ends the path,
     * and it is armed again by the next [watchTerminal], so a retained view shown again is timed too.
     */
    fun terminalFrame(id: Long) {
        if (!enabled) return
        val span = synchronized(lock) { terminalSpans.remove(id) } ?: return
        end(span, "frame")
    }

    /** The terminal is gone before it drew a frame. */
    fun forgetTerminal(id: Long) {
        synchronized(lock) { terminalSpans.remove(id) }
    }
}
