package io.github.code_akram.or2.terminal

import io.github.code_akram.or2.ffi.Scrollback
import io.github.code_akram.or2.ffi.TargetScroll
import io.github.code_akram.or2.ffi.TerminalModes
import io.github.code_akram.or2.ffi.TerminalTarget
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.launch

/** What a vertical swipe scrolls (contracts.md, "Wheel-aware scrolling"). */
enum class ScrollRoute {
    /** The program tracks the mouse: wheel events at the touched cell (`ViewportScroll.Wheel`). */
    WHEEL,

    /** tmux's own history: copy mode over exec (`scroll_target`). */
    TMUX,

    /** herdr's own history: `pane.scroll` (`scroll_target`). */
    HERDR,

    /** The local scrollback viewport (primary screen) or arrow keys (alternate screen). */
    VIEWPORT,
}

/**
 * The route for a swipe. A tmux target takes tmux's route whichever screen libghostty shows: tmux
 * draws on the alternate screen for its whole life over SSH, and mosh's server never relays the
 * alternate screen (only the mouse modes), so requiring it would leave tmux over mosh on a local
 * scrollback that holds mosh's redraws, not tmux's history.
 */
fun scrollRoute(modes: TerminalModes, target: TerminalTarget): ScrollRoute = when {
    modes.mouseTracking -> ScrollRoute.WHEEL
    target is TerminalTarget.Tmux -> ScrollRoute.TMUX
    target is TerminalTarget.Herdr -> ScrollRoute.HERDR
    else -> ScrollRoute.VIEWPORT
}

/** The primary-screen viewport is above the bottom of the scrollback (`offset` is its first row). */
fun viewportAway(scrollback: Scrollback, modes: TerminalModes, visibleRows: Int): Boolean =
    !modes.alternateScreen && visibleRows > 0 && scrollback.offset + visibleRows.toULong() < scrollback.totalRows

/**
 * The scroll-to-bottom button shows while the primary viewport is above its bottom or a tmux or
 * herdr target is scrolled up or may be ([TargetScroller.away]).
 */
fun scrollToBottomVisible(scrollback: Scrollback, modes: TerminalModes, visibleRows: Int, targetAway: Boolean): Boolean =
    targetAway || viewportAway(scrollback, modes, visibleRows)

/** The UTF-8 length of [text], without encoding it. */
fun utf8Length(text: CharSequence): Int {
    var bytes = 0
    var i = 0
    while (i < text.length) {
        val c = text[i]
        bytes += when {
            c.code < 0x80 -> 1
            c.code < 0x800 -> 2
            c.isHighSurrogate() && i + 1 < text.length && text[i + 1].isLowSurrogate() -> {
                i++
                4
            }
            else -> 3
        }
        i++
    }
    return bytes
}

/**
 * Scrolls a tmux or herdr target's own history through `scroll_target` ([send]): at most one call
 * in flight, the swipes in between summed into the next one. Tracks whether the target is scrolled
 * away from the bottom (the lines up minus the lines down, never below zero), and holds input while
 * it is or may be: an input first sends `Bottom` and goes out, exactly once and in order, only once a
 * `Bottom` has **succeeded**, so typing never lands in tmux's copy mode or herdr's history (owner
 * decision after the external v0.1.1 fix check).
 *
 * - A `Bottom` that fails or is cancelled, and an `Up` or `Down` that fails (it may have moved the
 *   target or not), leave the target [unconfirmed]: it counts as [away] (the button shows) and the
 *   next input needs a `Bottom` that succeeds.
 * - Input held behind a failed `Bottom` stays held, and `Bottom` is tried again after
 *   [RETRY_DELAYS_MS] (the last one repeating) while the host connection is [live]; while it is not,
 *   the input waits and the next try goes as soon as it is live again. Nothing gives up but [close]
 *   (the terminal closed), which drops what is held. [bottom] (the button) tries at once.
 * - A swipe up that went to tmux or herdr as wheel events (route 1, [wheeled]) leaves the target
 *   [unconfirmed] too: it scrolled itself, how far is unknown.
 * - At most [MAX_HELD_BYTES] are held: an input that would pass the cap is dropped (the newest, so
 *   what was typed first still goes out whole and in order) and [input] returns false.
 *
 * One per terminal (`ActiveTerminal.targetScroller`), not per view: what tmux or herdr shows outlives
 * any view of it (another terminal selected, Home, the view the SSH-to-mosh swap recreates), and
 * [scope] is the terminal holder's, so a call in flight is never cancelled by a view going away.
 * Main thread only; [send] runs in [scope].
 */
class TargetScroller(
    private val scope: CoroutineScope,
    private val send: suspend (TargetScroll) -> Unit,
    /** Whether the terminal's host connection is up now (a `StateFlow`-like flow: it starts with the current value). */
    private val live: Flow<Boolean> = flowOf(true),
    private val onAwayChanged: (Boolean) -> Unit = {},
) {
    private class Held(val bytes: Int, val send: () -> Unit)

    /** Rows not yet sent; negative is up (the `ViewportScroll` convention). */
    private var pendingRows = 0
    private var pendingBottom = false
    private var inFlight = false

    /** The call in flight (meaningful while [inFlight]). */
    private var inFlightCall: TargetScroll? = null

    /** The call in flight is a `Bottom`. */
    private val bottomInFlight get() = inFlight && inFlightCall == TargetScroll.Bottom
    private val held = ArrayDeque<Held>()
    private var heldBytes = 0

    /** The next `Bottom` try for held input after a failed one, waiting out its delay or the connection. */
    private var retry: Job? = null
    private val retrying get() = retry?.isActive == true

    /** `Bottom`s that failed in a row with input held: picks the next retry's delay. */
    private var failures = 0

    /** Lines the target is scrolled up from its bottom, as far as this terminal has asked. */
    var awayLines = 0L
        private set

    /** A call failed or was cancelled: the target may be in its history, how far unknown, until a `Bottom` succeeds. */
    var unconfirmed = false
        private set

    /** Wheel swipes up so far ([wheeled]): a `Bottom` sent before the newest one does not confirm the bottom. */
    private var wheelsUp = 0L

    /** The terminal has closed: nothing is held or sent any more. */
    var closed = false
        private set

    val away get() = awayLines > 0 || unconfirmed

    private val awayFlow = MutableStateFlow(false)

    /** [away] as a flow: a view shown again, or the new view of a swapped terminal, reads it at once. */
    val awayState: StateFlow<Boolean> = awayFlow.asStateFlow()

    /** Whether nothing is queued, in flight or held. */
    val idle get() = !inFlight && !pendingBottom && pendingRows == 0 && held.isEmpty()

    /** Bytes of input held now. */
    val heldSize get() = heldBytes

    /** A swipe of [rows] (negative = up). Down at the bottom has nothing to scroll and sends nothing. */
    fun scroll(rows: Int) {
        if (closed || rows == 0 || (rows > 0 && !away)) return
        changeAway { awayLines = (awayLines - rows).coerceAtLeast(0) }
        pendingRows += rows
        pump()
    }

    /**
     * A swipe of [rows] went to the program as wheel events (route 1: tmux with `mouse on`, herdr), so
     * tmux or herdr may have scrolled its own history by an amount this terminal cannot know. Up marks
     * the target [unconfirmed] (away, how far unknown): the button shows and the next input sends
     * `Bottom` first. Down changes nothing, even if it may have reached the bottom: only a `Bottom`
     * that succeeds clears it. Nothing is sent here; the wheel events already went out.
     */
    fun wheeled(rows: Int) {
        if (closed || rows >= 0) return
        wheelsUp++
        changeAway { unconfirmed = true }
    }

    /**
     * Back to the live screen, now (the button; also any waiting retry); queued swipes are dropped.
     * Only a `Bottom` that succeeds clears [unconfirmed].
     */
    fun bottom() {
        if (closed) return
        cancelRetry()
        pendingRows = 0
        pendingBottom = true
        changeAway { awayLines = 0 }
        pump()
    }

    /**
     * The terminal is no longer shown (minimised, another terminal selected, the app left): back to the
     * live screen, best effort. A failure leaves the target [unconfirmed], so the next input still
     * sends `Bottom` first and the button shows when the terminal is shown again.
     */
    fun leave() {
        if (away || pendingRows != 0) bottom()
    }

    /**
     * Runs [input] now when the target is at its bottom with nothing in flight; otherwise holds it until
     * a `Bottom` has succeeded. [bytes] is what it sends, for the cap: past [MAX_HELD_BYTES] the input is
     * dropped and this returns false (it is the newest; dropping it keeps what came before whole).
     */
    fun input(bytes: Int = 1, input: () -> Unit): Boolean {
        if (closed || (idle && !away)) {
            input()
            return true
        }
        if (heldBytes + bytes > MAX_HELD_BYTES) return false
        held.addLast(Held(bytes, input))
        heldBytes += bytes
        when {
            retrying -> Unit // A Bottom failed: its retry (or the button) sends the next.
            // Unconfirmed stays away while its Bottom is out: that Bottom (or what follows it) decides.
            (away || pendingRows != 0) && !(bottomInFlight && pendingRows == 0) -> bottom()
            else -> pump()
        }
        return true
    }

    /** The terminal closed: stop retrying and drop what is held (there is no session left to type into). */
    fun close() {
        closed = true
        cancelRetry()
        held.clear()
        heldBytes = 0
        pendingRows = 0
        pendingBottom = false
    }

    private inline fun changeAway(change: () -> Unit) {
        val was = away
        change()
        if (was != away) {
            awayFlow.value = away
            onAwayChanged(away)
        }
    }

    private fun pump() {
        if (inFlight || closed) return
        val next = when {
            pendingBottom -> TargetScroll.Bottom
            pendingRows < 0 -> TargetScroll.Up((-pendingRows).toUInt())
            pendingRows > 0 -> TargetScroll.Down(pendingRows.toUInt())
            else -> {
                settle()
                return
            }
        }
        pendingBottom = false
        pendingRows = 0
        inFlight = true
        inFlightCall = next
        val wheelsBefore = wheelsUp
        scope.launch {
            var succeeded = false
            try {
                send(next)
                succeeded = true
            } catch (error: CancellationException) {
                inFlight = false
                failed(next)
                throw error
            } catch (_: Exception) {
                // Handled below: the target may or may not have moved.
            } finally {
                inFlight = false
            }
            if (succeeded) {
                // A wheel swipe up while this Bottom was out may have scrolled the target again.
                if (next == TargetScroll.Bottom && wheelsUp == wheelsBefore) {
                    failures = 0
                    changeAway { unconfirmed = false }
                }
            } else {
                failed(next)
            }
            pump()
        }
    }

    /**
     * A call failed or was cancelled. Unless a `Bottom` is already queued behind it (which decides), the
     * target's position is unknown; input held behind a failed `Bottom` waits for a retry.
     */
    private fun failed(call: TargetScroll) {
        if (closed || pendingBottom) return
        changeAway { unconfirmed = true }
        if (call == TargetScroll.Bottom && held.isNotEmpty()) scheduleRetry()
    }

    /** Nothing left to send: held input goes out at the bottom; away (or unconfirmed), it needs a `Bottom` first. */
    private fun settle() {
        if (held.isEmpty()) return
        if (away) {
            if (!retrying) bottom()
            return
        }
        failures = 0
        while (held.isNotEmpty()) {
            val next = held.removeFirst()
            heldBytes -= next.bytes
            next.send()
        }
    }

    private fun scheduleRetry() {
        if (closed || retrying) return
        val wait = RETRY_DELAYS_MS[failures.coerceAtMost(RETRY_DELAYS_MS.lastIndex)]
        failures++
        retry = scope.launch {
            delay(wait)
            if (!live.first()) {
                live.first { it }
                failures = 0 // Back after a drop: the next failures start the delays again.
            }
            retry = null
            if (!closed && held.isNotEmpty()) bottom()
        }
    }

    private fun cancelRetry() {
        retry?.cancel()
        retry = null
    }

    companion object {
        /** Waits before each `Bottom` retry for held input; the last repeats while the connection is live. */
        val RETRY_DELAYS_MS = longArrayOf(250, 1_000, 2_000, 4_000)

        /** Input held behind a target's `Bottom`, at most (UTF-8 bytes, a key counted as [KEY_BYTES]). */
        const val MAX_HELD_BYTES = 64 * 1024

        /** What one key counts for against [MAX_HELD_BYTES] (its escape sequence is at most a few bytes). */
        const val KEY_BYTES = 8
    }
}
