package io.github.code_akram.or2.host

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.ffi.HostCapabilities
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.TmuxSession
import io.github.code_akram.or2.inbox.LinkStatus
import io.github.code_akram.or2.inbox.linkStatus
import io.github.code_akram.or2.session.HostTrustDialog
import io.github.code_akram.or2.session.hostStateMessage

/** The tmux session list of a connected host as the screen last learned it. */
sealed interface TmuxList {
    data object Loading : TmuxList
    data class Loaded(val sessions: List<TmuxSession>) : TmuxList
    data class Failed(val message: String) : TmuxList
}

/** Mirrors the Rust rules: nonempty, at most 128 bytes, no control characters, `\`, `:` or `.`. */
fun tmuxNameError(name: String): String? = when {
    name.isEmpty() -> "Enter a session name."
    name.toByteArray(Charsets.UTF_8).size > 128 -> "Use at most 128 bytes."
    name.any { it.isISOControl() } -> "Remove control characters."
    name.any { it == '\\' || it == ':' || it == '.' } -> "Remove backslash, colon and dot characters."
    else -> null
}

/**
 * One host: its connection, a shell, tmux sessions (attach, or create by name) and herdr
 * sessions. Stateless: the caller owns the connection and supplies what it knows.
 */
@Composable
fun HostScreen(
    host: Host,
    hostState: HostState?,
    caps: HostCapabilities?,
    capsError: String?,
    tmux: TmuxList,
    busy: Boolean,
    connect: () -> Unit,
    disconnect: () -> Unit,
    approve: (HostState.AwaitingHostKeyDecision) -> Unit,
    reject: () -> Unit,
    openShell: () -> Unit,
    openTmux: (String) -> Unit,
    openHerdr: (session: String?) -> Unit,
    refresh: () -> Unit,
    modifier: Modifier = Modifier,
    openTerminals: @Composable () -> Unit = {},
) {
    val link = linkStatus(hostState)
    Column(modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text(host.label, style = MaterialTheme.typography.titleLarge)
        Text("${host.username}@" + host.addressSummary, style = MaterialTheme.typography.bodySmall)
        Text(hostState?.let(::hostStateMessage) ?: LinkStatus.NOT_CONNECTED.label,
            color = if (link == LinkStatus.FAILED) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurface,
            modifier = Modifier.testTag("host-state"))
        (hostState as? HostState.Connected)?.let { connected ->
            if (host.addresses.size > 1) Text("Using address ${connected.addressIndex + 1u}: " +
                host.addresses.getOrNull(connected.addressIndex.toInt())?.let { "${it.hostname}:${it.port}" }.orEmpty(),
                style = MaterialTheme.typography.bodySmall)
        }
        Row {
            when (link) {
                LinkStatus.NOT_CONNECTED, LinkStatus.FAILED ->
                    Button(onClick = connect, enabled = !busy && host.keyId != null, modifier = Modifier.testTag("host-connect")) {
                        Text(if (host.keyId == null) "Select a key first" else "Unlock and connect")
                    }
                else -> TextButton(onClick = disconnect, modifier = Modifier.testTag("host-disconnect")) {
                    Text(if (link == LinkStatus.CONNECTED) "Disconnect" else "Cancel")
                }
            }
        }
        openTerminals()
        if (link == LinkStatus.CONNECTED) {
            Section("Shell") {
                Button(onClick = openShell, modifier = Modifier.testTag("host-shell")) { Text("Open shell") }
            }
            TmuxSection(caps, capsError, tmux, openTmux, refresh)
            HerdrSection(caps, openHerdr)
        }
    }
    (hostState as? HostState.AwaitingHostKeyDecision)?.let { prompt ->
        HostTrustDialog(prompt, busy, { approve(prompt) }, reject)
    }
}

@Composable
private fun Section(title: String, content: @Composable () -> Unit) {
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Text(title, style = MaterialTheme.typography.titleMedium)
        content()
    }
}

@Composable
private fun TmuxSection(caps: HostCapabilities?, capsError: String?, tmux: TmuxList, open: (String) -> Unit, refresh: () -> Unit) {
    Section("tmux") {
        when {
            capsError != null -> Text("Could not query the host. Try Refresh.")
            caps == null -> Text("Checking the host…")
            caps.tmux == null -> Text("tmux is not installed on this host.", modifier = Modifier.testTag("tmux-missing"))
            else -> {
                when (tmux) {
                    TmuxList.Loading -> Text("Loading sessions…")
                    is TmuxList.Failed -> Text(tmux.message, color = MaterialTheme.colorScheme.error)
                    is TmuxList.Loaded -> {
                        if (tmux.sessions.isEmpty()) Text("No tmux sessions yet.")
                        tmux.sessions.forEach { session ->
                            Card(Modifier.fillMaxWidth().testTag("tmux:${session.name}")) {
                                Row(Modifier.padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                                    Column(Modifier.weight(1f).padding(vertical = 8.dp)) {
                                        Text(session.name, style = MaterialTheme.typography.titleSmall)
                                        Text("${session.windows} window${if (session.windows == 1u) "" else "s"}" +
                                            if (session.attachedClients > 0u) " · ${session.attachedClients} attached" else "",
                                            style = MaterialTheme.typography.bodySmall)
                                    }
                                    TextButton(onClick = { open(session.name) }, modifier = Modifier.testTag("tmux-attach:${session.name}")) { Text("Attach") }
                                }
                            }
                        }
                    }
                }
                NewTmuxSession(open)
            }
        }
        TextButton(onClick = refresh, modifier = Modifier.testTag("host-refresh")) { Text("Refresh") }
    }
}

@Composable
private fun NewTmuxSession(open: (String) -> Unit) {
    var name by rememberSaveable { mutableStateOf("") }
    // Show the rule only once something is typed, so the empty field is not an error.
    val error = if (name.isEmpty()) null else tmuxNameError(name)
    Row(verticalAlignment = Alignment.Top, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        OutlinedTextField(name, { name = it }, label = { Text("New session name") }, singleLine = true,
            isError = error != null, supportingText = { error?.let { Text(it) } },
            modifier = Modifier.weight(1f).testTag("tmux-new-name"))
        Button(onClick = { open(name); name = "" }, enabled = tmuxNameError(name) == null,
            modifier = Modifier.padding(top = 8.dp).testTag("tmux-new")) { Text("Create") }
    }
}

@Composable
private fun HerdrSection(caps: HostCapabilities?, open: (String?) -> Unit) {
    Section("herdr") {
        when {
            caps == null -> Text("Checking the host…")
            caps.herdr == null -> Text("herdr is not installed on this host.", modifier = Modifier.testTag("herdr-missing"))
            caps.herdrSessions.isEmpty() -> Text("No herdr sessions.")
            else -> caps.herdrSessions.forEach { session ->
                Card(Modifier.fillMaxWidth().testTag("herdr:${session.name}")) {
                    Row(Modifier.padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f).padding(vertical = 8.dp)) {
                            Text(session.name + if (session.isDefault) " (default)" else "", style = MaterialTheme.typography.titleSmall)
                            Text(if (session.running) "Running" else "Not running", style = MaterialTheme.typography.bodySmall)
                        }
                        // The default session is opened without a name, never by its listed name.
                        TextButton(onClick = { open(if (session.isDefault) null else session.name) }, enabled = session.running,
                            modifier = Modifier.testTag("herdr-open:${session.name}")) { Text("Open") }
                    }
                }
            }
        }
    }
}
