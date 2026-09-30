package io.github.code_akram.or2.hosts

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import io.github.code_akram.or2.data.HostRecord
import io.github.code_akram.or2.data.KeyRecord

@Composable
fun HostsScreen(
    hosts: List<HostRecord>, keys: List<KeyRecord>, busy: Boolean,
    save: (HostRecord, HostRecord?) -> Unit, delete: (HostRecord) -> Unit, connect: (HostRecord) -> Unit,
) {
    var editing by remember { mutableStateOf(false) }
    var previous by remember { mutableStateOf<HostRecord?>(null) }
    var deleting by remember { mutableStateOf<HostRecord?>(null) }
    Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Button(onClick = { previous = null; editing = true }, enabled = !busy) { Text("Add host") }
        if (hosts.isEmpty()) Text("No hosts yet. Add a key, then add a host. Connections require explicit host-key trust.")
        hosts.forEach { host ->
            Card(Modifier.fillMaxWidth()) {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text(host.label)
                    Text("${host.username}@${host.hostname}:${host.port}")
                    Text("Key: ${keys.find { it.id == host.keyId }?.label ?: "Select a key"}")
                    Row {
                        TextButton(onClick = { connect(host) }, enabled = !busy && host.keyId != null) { Text("Connect") }
                        TextButton(onClick = { previous = host; editing = true }, enabled = !busy) { Text("Edit") }
                        TextButton(onClick = { deleting = host }, enabled = !busy) { Text("Delete") }
                    }
                }
            }
        }
    }
    if (editing) HostDialog(previous, keys, dismiss = { editing = false }) { host ->
        save(host, previous)
        editing = false
    }
    deleting?.let { host ->
        AlertDialog(onDismissRequest = { deleting = null }, title = { Text("Delete host?") },
            text = { Text("Delete ${host.label} and its trusted host keys? This does not delete your SSH key.") },
            confirmButton = { TextButton(onClick = { delete(host); deleting = null }) { Text("Delete") } },
            dismissButton = { TextButton(onClick = { deleting = null }) { Text("Cancel") } })
    }
}

fun hostFieldError(value: String): String? = when {
    value.isBlank() -> "Enter a value."
    value.trim().any { it.isWhitespace() || it.isISOControl() } -> "Remove internal whitespace or control characters."
    else -> null
}

fun validHost(label: String, hostname: String, port: String, username: String): Boolean =
    label.isNotBlank() && hostFieldError(hostname) == null &&
        port.toIntOrNull() in 1..65535 && hostFieldError(username) == null

@Composable
private fun HostDialog(previous: HostRecord?, keys: List<KeyRecord>, dismiss: () -> Unit, save: (HostRecord) -> Unit) {
    var label by remember { mutableStateOf(previous?.label ?: "") }
    var hostname by remember { mutableStateOf(previous?.hostname ?: "") }
    var port by remember { mutableStateOf(previous?.port?.toString() ?: "22") }
    var username by remember { mutableStateOf(previous?.username ?: "") }
    var keyId by remember { mutableStateOf(if (previous == null) keys.singleOrNull()?.id else previous.keyId) }
    val hostnameError = hostFieldError(hostname)
    val usernameError = hostFieldError(username)
    AlertDialog(onDismissRequest = dismiss, title = { Text(if (previous == null) "Add host" else "Edit host") },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(label, { label = it }, label = { Text("Label") }, singleLine = true)
                OutlinedTextField(hostname, { hostname = it }, label = { Text("Hostname") }, singleLine = true,
                    isError = hostnameError != null, supportingText = { hostnameError?.let { Text(it) } })
                OutlinedTextField(port, { port = it }, label = { Text("Port (1–65535)") }, singleLine = true)
                OutlinedTextField(username, { username = it }, label = { Text("Username") }, singleLine = true,
                    isError = usernameError != null, supportingText = { usernameError?.let { Text(it) } })
                Text("SSH key")
                if (keys.none { it.id == keyId }) Text("Choose a key", color = MaterialTheme.colorScheme.error)
                if (keys.isEmpty()) Text("Generate or import a key on the Keys tab first.")
                Column(Modifier.selectableGroup()) {
                    keys.forEach { key ->
                        Row(Modifier.fillMaxWidth().heightIn(min = 48.dp)
                            .selectable(selected = keyId == key.id, role = Role.RadioButton, onClick = { keyId = key.id }),
                            verticalAlignment = Alignment.CenterVertically) {
                            RadioButton(selected = keyId == key.id, onClick = null)
                            Text(key.label)
                        }
                    }
                }
                if (previous != null) Text("Changing the hostname or port clears previous host-key trust.")
            }
        },
        confirmButton = {
            TextButton(onClick = { save(HostRecord(previous?.id ?: 0, label.trim(), hostname, port.toInt(), username, keyId)) },
                enabled = validHost(label, hostname, port, username) && keys.any { it.id == keyId }) { Text("Save") }
        },
        dismissButton = { TextButton(onClick = dismiss) { Text("Cancel") } })
}
