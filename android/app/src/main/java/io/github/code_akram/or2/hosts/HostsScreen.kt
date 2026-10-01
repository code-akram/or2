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
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.HostEndpoint
import io.github.code_akram.or2.data.HostRecord
import io.github.code_akram.or2.data.KeyRecord
import io.github.code_akram.or2.inbox.LinkStatus

@Composable
fun HostsScreen(
    hosts: List<Host>, keys: List<KeyRecord>, busy: Boolean,
    save: (Host, Host?) -> Unit, delete: (Host) -> Unit, connect: (Host) -> Unit,
    open: (Host) -> Unit = {}, link: (Host) -> LinkStatus = { LinkStatus.NOT_CONNECTED },
) {
    var editing by remember { mutableStateOf(false) }
    var previous by remember { mutableStateOf<Host?>(null) }
    var deleting by remember { mutableStateOf<Host?>(null) }
    Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Button(onClick = { previous = null; editing = true }, enabled = !busy) { Text("Add host") }
        if (hosts.isEmpty()) Text("No hosts yet. Add a key, then add a host. Connections require explicit host-key trust.")
        hosts.forEach { host ->
            val status = link(host)
            Card(Modifier.fillMaxWidth().testTag("host:${host.id}")) {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text(host.label, style = MaterialTheme.typography.titleMedium)
                    Text("${host.username}@${host.addresses.first().hostname}:${host.addresses.first().port}")
                    if (host.addresses.size > 1) Text("Also: " + host.addresses.drop(1).joinToString(", ") { "${it.hostname}:${it.port}" },
                        style = MaterialTheme.typography.bodySmall)
                    Text("Key: ${keys.find { it.id == host.keyId }?.label ?: "Select a key"}")
                    Text(if (host.showInInbox) "In the inbox" else "Hidden from the inbox", style = MaterialTheme.typography.bodySmall)
                    Text(status.label, style = MaterialTheme.typography.labelMedium)
                    Row {
                        TextButton(onClick = { open(host) }) { Text("Open") }
                        TextButton(onClick = { connect(host) }, enabled = !busy && host.keyId != null &&
                            (status == LinkStatus.NOT_CONNECTED || status == LinkStatus.FAILED)) { Text("Connect") }
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

@Composable
private fun HostDialog(previous: Host?, keys: List<KeyRecord>, dismiss: () -> Unit, save: (Host) -> Unit) {
    var label by remember { mutableStateOf(previous?.label ?: "") }
    var addresses by remember {
        mutableStateOf(previous?.addresses?.map(AddressDraft::of) ?: listOf(AddressDraft("", "22")))
    }
    var username by remember { mutableStateOf(previous?.username ?: "") }
    var keyId by remember { mutableStateOf(if (previous == null) keys.singleOrNull()?.id else previous.keyId) }
    var showInInbox by remember { mutableStateOf(previous?.showInInbox ?: true) }
    val usernameError = hostFieldError(username)
    AlertDialog(onDismissRequest = dismiss, title = { Text(if (previous == null) "Add host" else "Edit host") },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(label, { label = it }, label = { Text("Label") }, singleLine = true)
                Text("Addresses, in order of preference. All are tried; the first to answer wins.",
                    style = MaterialTheme.typography.bodySmall)
                addresses.forEachIndexed { index, address ->
                    val hostnameError = hostFieldError(address.hostname)
                    Column(Modifier.testTag("address:$index"), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                        OutlinedTextField(address.hostname, { addresses = addresses.toMutableList().also { list -> list[index] = address.copy(hostname = it) } },
                            label = { Text("Hostname") }, singleLine = true, modifier = Modifier.testTag("address-hostname:$index"),
                            isError = hostnameError != null, supportingText = { hostnameError?.let { Text(it) } })
                        OutlinedTextField(address.port, { addresses = addresses.toMutableList().also { list -> list[index] = address.copy(port = it) } },
                            label = { Text("Port (1–65535)") }, singleLine = true, modifier = Modifier.testTag("address-port:$index"),
                            isError = portError(address.port) != null)
                        Row {
                            TextButton(onClick = { addresses = addresses.moved(index, -1) }, enabled = index > 0,
                                modifier = Modifier.testTag("address-up:$index")) { Text("Move up") }
                            TextButton(onClick = { addresses = addresses.moved(index, 1) }, enabled = index < addresses.lastIndex,
                                modifier = Modifier.testTag("address-down:$index")) { Text("Move down") }
                            TextButton(onClick = { addresses = addresses.filterIndexed { i, _ -> i != index } }, enabled = addresses.size > 1,
                                modifier = Modifier.testTag("address-remove:$index")) { Text("Remove") }
                        }
                    }
                }
                TextButton(onClick = { addresses = addresses + AddressDraft("", "22") }, enabled = addresses.size < Host.MAX_ADDRESSES,
                    modifier = Modifier.testTag("address-add")) { Text("Add address") }
                OutlinedTextField(username, { username = it }, label = { Text("Username") }, singleLine = true,
                    isError = usernameError != null, supportingText = { usernameError?.let { Text(it) } })
                Row(Modifier.fillMaxWidth().heightIn(min = 48.dp), verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.SpaceBetween) {
                    Text("Show agents in the inbox")
                    Switch(showInInbox, { showInInbox = it }, modifier = Modifier.testTag("host-inbox"))
                }
                Text("SSH key")
                if (keys.none { it.id == keyId }) Text("Choose a key", color = MaterialTheme.colorScheme.error)
                if (keys.isEmpty()) Text("Generate or import a key on the Keys tab first.")
                Column(Modifier.selectableGroup()) {
                    keys.forEach { key ->
                        Row(Modifier.fillMaxWidth().heightIn(min = 48.dp)
                            .testTag("host-key:${key.id}")
                            .semantics(mergeDescendants = true) {}
                            .selectable(selected = keyId == key.id, role = Role.RadioButton, onClick = { keyId = key.id }),
                            verticalAlignment = Alignment.CenterVertically) {
                            RadioButton(selected = keyId == key.id, onClick = null)
                            Text(key.label)
                        }
                    }
                }
                if (previous != null) Text("Changing any address or port clears previous host-key trust.")
            }
        },
        confirmButton = {
            TextButton(onClick = {
                save(Host(
                    HostRecord(previous?.id ?: 0, label.trim(), username, keyId, showInInbox),
                    addresses.map { HostEndpoint(it.hostname, it.port.toInt()) },
                ))
            }, enabled = validHost(label, addresses, username) && keys.any { it.id == keyId }) { Text("Save") }
        },
        dismissButton = { TextButton(onClick = dismiss) { Text("Cancel") } })
}
