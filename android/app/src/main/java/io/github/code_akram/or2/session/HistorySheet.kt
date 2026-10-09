package io.github.code_akram.or2.session

import android.graphics.Typeface
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import io.github.code_akram.or2.ffi.HistoryText
import io.github.code_akram.or2.ffi.HostException
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.terminal.terminalTypefaces
import io.github.code_akram.or2.ui.Or2Colors
import io.github.code_akram.or2.ui.Or2Dimens
import io.github.code_akram.or2.ui.Or2Shapes
import io.github.code_akram.or2.ui.Or2Sheet
import io.github.code_akram.or2.ui.Or2Type
import io.github.code_akram.or2.ui.TextAction

/** How many lines the history sheet asks for (`read_history` takes 1 to 5000). */
const val HISTORY_LINES: UInt = 2000u

/**
 * Whether a terminal on [target] has a history the host can read (the Terminals sheet's **History**, the mosh chip):
 * tmux and herdr keep one, on SSH and mosh alike; a plain shell does not.
 */
fun hasHistory(target: TerminalTarget): Boolean = target is TerminalTarget.Tmux || target is TerminalTarget.Herdr

/** What the history sheet shows: the read in flight, its text, or why it failed. */
sealed interface HistoryLoad {
    data object Loading : HistoryLoad

    /** [text] as shown and copied ([sanitizeHistory]); [truncated]: older lines exist that are not in it. */
    data class Loaded(val text: String, val lineCount: Int, val truncated: Boolean) : HistoryLoad

    data class Failed(val message: String) : HistoryLoad
}

/** The sheet's state for what `read_history` answered. */
fun historyLoaded(history: HistoryText): HistoryLoad.Loaded {
    val text = sanitizeHistory(history.text)
    return HistoryLoad.Loaded(text, if (text.isEmpty()) 0 else text.count { it == '\n' } + 1, history.truncated)
}

/**
 * [text] as the sheet shows it: control characters other than tab removed (C0, DEL and C1; a stray escape or bell
 * must not reach the clipboard), line breaks kept, the trailing blanks of each line and the blank lines at the end
 * (the empty rows under a prompt) dropped.
 */
fun sanitizeHistory(text: String): String {
    val kept = StringBuilder(text.length)
    for (c in text) {
        if (c == '\n' || c == '\t' || !(c < ' ' || c in '\u007f'..'\u009f')) kept.append(c)
    }
    return kept.lines().joinToString("\n") { it.trimEnd(' ', '\t') }.trimEnd('\n')
}

/** Why a history read failed, in the sheet's words: a pane that has gone or a failed read say so, the rest as elsewhere. */
fun historyErrorMessage(error: HostException): String = when (error) {
    is HostException.PaneNotFound -> "This herdr pane no longer exists."
    is HostException.CommandFailed -> "The host could not read the history. Retry, or reconnect."
    else -> hostErrorMessage(error)
}

/** The line under the title: `2000 lines`, with `· older lines not shown` when the read did not reach the start. */
fun historyNote(loaded: HistoryLoad.Loaded): String {
    val count = when (loaded.lineCount) {
        0 -> "No lines"
        1 -> "1 line"
        else -> "${loaded.lineCount} lines"
    }
    return if (loaded.truncated) "$count · older lines not shown" else count
}

/** What **Copy all** puts on the clipboard: the text as shown, or nothing while there is none. */
fun historyCopyText(load: HistoryLoad): String? = (load as? HistoryLoad.Loaded)?.text?.takeIf { it.isNotEmpty() }

/**
 * The history sheet (the Terminals sheet's **History**, or the mosh chip): a tmux or herdr target's history as read
 * from the host, full height, in the terminal's font, newest at the bottom and opened there, selectable, with **Copy
 * all** ([copyAll] with [historyCopyText]) and the line count ([historyNote]). Reading it moved nothing on the host.
 * Loading and a failure (with **Retry**) show inline, where the text goes.
 */
@Composable
fun HistorySheet(title: String, load: HistoryLoad, copyAll: (String) -> Unit, retry: () -> Unit, dismiss: () -> Unit) {
    Or2Sheet(dismiss, title = "History", subtitle = title, scrollable = false, modifier = Modifier.fillMaxHeight().testTag("history-sheet")) {
        Column(Modifier.fillMaxSize().padding(start = Or2Dimens.Gutter, end = Or2Dimens.Gutter, bottom = Or2Dimens.Gutter)) {
            Row(Modifier.fillMaxWidth().padding(start = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(
                    (load as? HistoryLoad.Loaded)?.let(::historyNote).orEmpty(), style = Or2Type.Secondary, color = Or2Colors.TextMuted,
                    maxLines = 1, modifier = Modifier.weight(1f).testTag("history-count"),
                )
                val copyable = historyCopyText(load)
                TextAction("Copy all", { copyable?.let(copyAll) }, Modifier.testTag("history-copy-all"), enabled = copyable != null)
            }
            Spacer(Modifier.height(4.dp))
            Box(
                Modifier.weight(1f).fillMaxWidth().clip(Or2Shapes.Field).background(Or2Colors.TerminalBackground),
            ) {
                when (load) {
                    HistoryLoad.Loading -> Text(
                        "Reading history…", style = Or2Type.Mono, color = Or2Colors.TextMuted,
                        modifier = Modifier.padding(12.dp).testTag("history-loading"),
                    )
                    is HistoryLoad.Failed -> Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                        Text(load.message, style = Or2Type.Body, color = Or2Colors.Danger, modifier = Modifier.testTag("history-error"))
                        TextAction("Retry", retry, Modifier.testTag("history-retry"))
                    }
                    is HistoryLoad.Loaded -> if (load.text.isEmpty()) {
                        Text(
                            "Nothing has scrolled by yet.", style = Or2Type.Mono, color = Or2Colors.TextMuted,
                            modifier = Modifier.padding(12.dp).testTag("history-empty"),
                        )
                    } else {
                        HistoryBody(load.text)
                    }
                }
            }
        }
    }
}

/** The text itself: selectable, in the terminal's font, scrolled from the bottom (the newest line) up. */
@Composable
private fun HistoryBody(text: String) {
    val context = LocalContext.current
    val font = remember(context) { FontFamily(terminalTypefaces(context)[Typeface.NORMAL]) }
    // Reverse scrolling: offset 0 is the bottom, so the sheet opens on the newest line without waiting for a layout.
    val scroll = rememberScrollState()
    SelectionContainer(Modifier.fillMaxSize().verticalScroll(scroll, reverseScrolling = true)) {
        Text(
            text, style = Or2Type.Mono.copy(fontFamily = font), color = Or2Colors.Text,
            modifier = Modifier.fillMaxWidth().padding(horizontal = 10.dp, vertical = 8.dp).testTag("history-text"),
        )
    }
}
