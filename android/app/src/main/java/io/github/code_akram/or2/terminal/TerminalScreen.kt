package io.github.code_akram.or2.terminal

import android.content.ClipboardManager
import android.graphics.RectF
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.LayoutCoordinates
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInRoot
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import io.github.code_akram.or2.ffi.SessionInterface
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalKey
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch

@Composable
fun TerminalScreen(
    session: SessionInterface,
    state: StateFlow<SessionState>,
    frameReady: Flow<Unit>,
    modifier: Modifier = Modifier,
) {
    key(session) {
        val context = LocalContext.current
        val view = remember(session, context) { TerminalView(context).apply { bind(session) } }
        var ctrl by remember { mutableStateOf(false) }
        var alt by remember { mutableStateOf(false) }
        var selecting by remember { mutableStateOf(false) }
        var pendingPaste by remember { mutableStateOf<String?>(null) }
        DisposableEffect(view) {
            view.onInputChanged = { ctrl = view.input.ctrl; alt = view.input.alt }
            view.onSelectionChanged = { selecting = view.selection != null }
            onDispose {
                view.onInputChanged = {}
                view.onSelectionChanged = {}
            }
        }
        LaunchedEffect(view, state, frameReady) {
            launch { state.collect { view.sessionState(it) } }
            launch { frameReady.collect { view.frameReady() } }
        }
        Column(modifier = modifier.fillMaxSize().imePadding()) {
            AndroidView(factory = { view }, modifier = Modifier.weight(1f).clipToBounds())
            Surface(color = Color(0xff101018), contentColor = Color(0xffd8e8ff)) {
                Column {
                    Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).onGloballyPositioned {
                        view.primaryKeyRowBounds = it.unclippedBoundsInRoot()
                    }.semantics {
                        contentDescription = "Terminal primary keys"
                    }) {
                        fun keyModifier(label: String) = Modifier.weight(1f).onGloballyPositioned {
                            view.primaryKeyBounds[label] = it.unclippedBoundsInRoot()
                        }
                        TerminalButton("Esc", modifier = keyModifier("Esc")) { view.input.key(TerminalKey.Escape) }
                        TerminalButton("Tab", modifier = keyModifier("Tab")) { view.input.key(TerminalKey.Tab) }
                        TerminalButton("Ctrl", ctrl, keyModifier("Ctrl")) { view.input.toggleCtrl() }
                        TerminalButton("Alt", alt, keyModifier("Alt")) { view.input.toggleAlt() }
                        listOf("←" to TerminalKey.ArrowLeft, "↓" to TerminalKey.ArrowDown,
                            "↑" to TerminalKey.ArrowUp, "→" to TerminalKey.ArrowRight).forEach { (label, key) ->
                            TerminalButton(label, modifier = keyModifier(label)) { view.input.key(key) }
                        }
                    }
                    Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).onGloballyPositioned {
                        view.actionRowBounds = it.unclippedBoundsInRoot()
                    }) {
                        // These actions stay fixed even when navigation/symbol extras scroll.
                        fun actionModifier(label: String) = Modifier.onGloballyPositioned {
                            view.actionBounds[label] = it.unclippedBoundsInRoot()
                        }
                        if (selecting) {
                            TerminalButton("Copy", modifier = actionModifier("Copy")) { view.copySelection() }
                            TerminalButton("Clear", modifier = actionModifier("Clear")) { view.clearSelection() }
                        }
                        TerminalButton("Paste", modifier = actionModifier("Paste")) {
                            val text = context.getSystemService(ClipboardManager::class.java).primaryClip
                                ?.getItemAt(0)?.text?.toString().orEmpty()
                            if (pasteNeedsConfirmation(text)) pendingPaste = text else view.paste(text)
                        }
                        Row(Modifier.weight(1f).horizontalScroll(rememberScrollState())) {
                            TerminalButton("Keyboard") { view.showKeyboard() }
                            TerminalButton("Bottom") { view.jumpToBottom() }
                            listOf("Home" to TerminalKey.Home, "End" to TerminalKey.End,
                                "PgUp" to TerminalKey.PageUp, "PgDn" to TerminalKey.PageDown).forEach { (label, key) ->
                                TerminalButton(label) { view.input.key(key) }
                            }
                            listOf("/", "-", "|", "~", "_", "$", "&", "*", "{", "}", "(", ")", "[", "]", "=", ";", "'", "\"").forEach { symbol ->
                                TerminalButton(symbol) { view.input.key(TerminalKey.Character(symbol)) }
                            }
                        }
                    }
                }
            }
        }
        pendingPaste?.let { text ->
            AlertDialog(
                onDismissRequest = { pendingPaste = null },
                title = { Text("Paste ${pasteLineCount(text)} lines?") },
                text = { Text("They will run as typed.") },
                confirmButton = { TerminalButton("Paste") { pendingPaste = null; view.paste(text) } },
                dismissButton = { TerminalButton("Cancel") { pendingPaste = null } },
                containerColor = Color(0xff101018), titleContentColor = Color(0xffd8e8ff),
                textContentColor = Color(0xffd8e8ff),
            )
        }
    }
}

// Position + measured size, not boundsInRoot(), so clipping cannot hide a partial key.
private fun LayoutCoordinates.unclippedBoundsInRoot(): RectF {
    val position = positionInRoot()
    return RectF(position.x, position.y, position.x + size.width, position.y + size.height)
}

@Composable
private fun TerminalButton(label: String, selected: Boolean = false, modifier: Modifier = Modifier, action: () -> Unit) {
    TextButton(onClick = action, modifier = modifier.heightIn(min = 48.dp).semantics {
        if (label == "Ctrl" || label == "Alt") stateDescription = if (selected) "Armed for next key" else "Off"
    }, contentPadding = PaddingValues(horizontal = 4.dp), colors = ButtonDefaults.textButtonColors(
        containerColor = if (selected) Color(0xff23405b) else Color.Transparent,
        contentColor = if (selected) Color(0xff66ccff) else Color(0xffd8e8ff),
    )) {
        Text(label, maxLines = 1, softWrap = false)
    }
}
