package io.github.code_akram.or2.terminal

import io.github.code_akram.or2.ffi.Scrollback
import io.github.code_akram.or2.ffi.TargetScroll
import io.github.code_akram.or2.ffi.TerminalModes
import io.github.code_akram.or2.ffi.TerminalTarget
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
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
 * herdr target is scrolled up ([TargetScroller.away]).
 */
fun scrollToBottomVisible(scrollback: Scrollback, modes: TerminalModes, visibleRows: Int, targetAway: Boolean): Boolean =
    targetAway || viewportAway(scrollback, modes, visibleRows)

/**
 * Scrolls a tmux or herdr target's own history through `scroll_target` ([send]): at most one call
 * in flight, the swipes in between summed into the next one. Tracks whether the target is scrolled
 * away from the bottom (the lines up minus the lines down, never below zero), and holds input while
 * it is: an input first sends `Bottom` and goes out once that call has returned, so typing never
 * lands in tmux's copy mode.
 *
 * One per terminal (`ActiveTerminal.targetScroller`), not per view: what tmux or herdr shows outlives
 * any view of it (another terminal selected, Home, the view the SSH-to-mosh swap recreates), and
 * [scope] is the terminal holder's, so a call in flight is never cancelled by a view going away. A
 * `Bottom` that fails or is cancelled leaves the target [unconfirmed]: it still counts as [away], so
 * the button shows and the next input sends `Bottom` again first. Main thread only; [send] runs in
 * [scope].
 */
class TargetScroller(
    private val scope: CoroutineScope,
    private val send: suspend (TargetScroll) -> Unit,
    private val onAwayChanged: (Boolean) -> Unit = {},
) {
    /** Rows not yet sent; negative is up (the `ViewportScroll` convention). */
    private var pendingRows = 0
    private var pendingBottom = false
    private var inFlight = false
    private val held = ArrayDeque<() -> Unit>()

    /** Lines the target is scrolled up from its bottom, as far as this terminal has asked. */
    var awayLines = 0L
        private set

    /** The last `Bottom` failed or was cancelled: the target may still be in its history, how far unknown. */
    var unconfirmed = false
        private set

    val away get() = awayLines > 0 || unconfirmed

    private val awayFlow = MutableStateFlow(false)

    /** [away] as a flow: a view shown again, or the new view of a swapped terminal, reads it at once. */
    val awayState: StateFlow<Boolean> = awayFlow.asStateFlow()

    /** Whether nothing is queued, in flight or held. */
    val idle get() = !inFlight && !pendingBottom && pendingRows == 0 && held.isEmpty()

    /** A swipe of [rows] (negative = up). Down at the bottom has nothing to scroll and sends nothing. */
    fun scroll(rows: Int) {
        if (rows == 0 || (rows > 0 && !away)) return
        changeAway { awayLines = (awayLines - rows).coerceAtLeast(0) }
        pendingRows += rows
        pump()
    }

    /** Back to the live screen; queued swipes are dropped. */
    fun bottom() {
        pendingRows = 0
        pendingBottom = true
        changeAway {
            awayLines = 0
            unconfirmed = false
        }
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

    /** Runs [input] now when the target is at its bottom with nothing in flight; otherwise after a `Bottom`. */
    fun input(input: () -> Unit) {
        if (idle && !away) {
            input()
            return
        }
        held.addLast(input)
        if (away || pendingRows != 0) bottom() else pump()
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
        if (inFlight) return
        val next = when {
            pendingBottom -> TargetScroll.Bottom
            pendingRows < 0 -> TargetScroll.Up((-pendingRows).toUInt())
            pendingRows > 0 -> TargetScroll.Down(pendingRows.toUInt())
            else -> {
                releaseHeld()
                return
            }
        }
        pendingBottom = false
        pendingRows = 0
        inFlight = true
        scope.launch {
            var failed = false
            try {
                send(next)
            } catch (error: CancellationException) {
                inFlight = false
                if (next == TargetScroll.Bottom) bottomFailed()
                throw error
            } catch (_: Exception) {
                // A failed scroll leaves the target where it was; typing must still go out.
                failed = true
            } finally {
                inFlight = false
            }
            if (next == TargetScroll.Bottom) {
                if (failed) bottomFailed()
                releaseAfterBottom()
            }
            pump()
        }
    }

    /** Unless another `Bottom` is already queued, the target may still be in its history. */
    private fun bottomFailed() {
        if (!pendingBottom) changeAway { unconfirmed = true }
    }

    /**
     * A `Bottom` returned: what was held behind it goes out, also when it failed (typing is never
     * dropped; the target stays [unconfirmed] and the next input tries again). A swipe up since then
     * needs another `Bottom` first.
     */
    private fun releaseAfterBottom() {
        if (pendingBottom) return // The queued Bottom releases them.
        if (awayLines > 0 || pendingRows != 0) {
            if (held.isNotEmpty()) bottom()
            return
        }
        while (held.isNotEmpty()) held.removeFirst()()
    }

    /** Nothing left to send: input held behind a scroll in flight goes out, unless the target is away again. */
    private fun releaseHeld() {
        if (away) {
            if (held.isNotEmpty()) bottom()
            return
        }
        while (held.isNotEmpty()) held.removeFirst()()
    }
}
