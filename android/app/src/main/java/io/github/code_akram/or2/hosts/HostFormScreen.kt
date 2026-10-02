package io.github.code_akram.or2.hosts

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.listSaver
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.HostEndpoint
import io.github.code_akram.or2.data.HostRecord
import io.github.code_akram.or2.data.KeyRecord
import io.github.code_akram.or2.data.TransportPref
import io.github.code_akram.or2.keys.KeyPicker
import io.github.code_akram.or2.keys.newKeyComment
import io.github.code_akram.or2.keys.newKeyErrorMessage
import io.github.code_akram.or2.keys.newKeyLabel
import io.github.code_akram.or2.pair.PairInstallKeyScreen
import io.github.code_akram.or2.ui.BottomInsetSpacer
import io.github.code_akram.or2.ui.GroupCard
import io.github.code_akram.or2.ui.GroupDivider
import io.github.code_akram.or2.ui.IconAction
import io.github.code_akram.or2.ui.ListRow
import io.github.code_akram.or2.ui.Or2Colors
import io.github.code_akram.or2.ui.Or2Dimens
import io.github.code_akram.or2.ui.Or2Field
import io.github.code_akram.or2.ui.Or2Icons
import io.github.code_akram.or2.ui.Or2Toggle
import io.github.code_akram.or2.ui.Or2Type
import io.github.code_akram.or2.ui.PillButton
import io.github.code_akram.or2.ui.PrimaryButton
import io.github.code_akram.or2.ui.Segmented
import io.github.code_akram.or2.ui.TopBar
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import io.github.code_akram.or2.ui.scrolledUnder

/**
 * Add or edit a host: filled fields with labels above and mono placeholders, an ordered address
 * list (each with its own port), the key choice, the inbox toggle, a full-width pill and a
 * mirrored top-bar check. Stateless storage-wise: [save] gets the finished host, then the form ends with [saved]
 * (by default [close]; the app goes on to the battery step after a new host). [close] alone leaves without saving.
 *
 * The key choice ends with **New key**, as on the Easy pair review (preselected when the phone has no key): Save
 * first makes and stores it with [createKey] (the biometric prompt), selects it, saves the host with it and then
 * shows its public line to add to `authorized_keys` on the host.
 */
@Composable
fun HostFormScreen(
    previous: Host?, keys: List<KeyRecord>, busy: Boolean, save: (Host) -> Unit, close: () -> Unit,
    createKey: suspend (label: String, comment: String) -> KeyRecord, deviceLabel: String,
    saved: () -> Unit = close,
) {
    // Typed input survives rotation and process death, and is re-seeded when a different host is edited.
    val identity = previous?.id ?: 0L
    var label by rememberSaveable(identity) { mutableStateOf(previous?.label ?: "") }
    var addresses by rememberSaveable(identity, stateSaver = AddressDraftsSaver) {
        mutableStateOf(previous?.addresses?.map(AddressDraft::of) ?: listOf(AddressDraft("", "22")))
    }
    var username by rememberSaveable(identity) { mutableStateOf(previous?.username ?: "") }
    var keyId by rememberSaveable(identity) { mutableStateOf(if (previous == null) keys.singleOrNull()?.id else previous.keyId) }
    var newKey by rememberSaveable(identity) { mutableStateOf(previous == null && keys.isEmpty()) }
    var showInInbox by rememberSaveable(identity) { mutableStateOf(previous?.showInInbox ?: true) }
    var transport by rememberSaveable(identity) { mutableStateOf(previous?.transport ?: TransportPref.AUTO) }
    var sleeps by rememberSaveable(identity) { mutableStateOf(previous?.sleeps ?: false) }
    // Making the new key: its biometric prompt is up. Not saved: a recreated screen's prompt is gone with the old one.
    var working by remember { mutableStateOf(false) }
    var keyError by rememberSaveable(identity) { mutableStateOf<String?>(null) }
    // The host is saved with a key made here: the form shows that key's line to install on the host.
    var installKeyId by rememberSaveable(identity) { mutableStateOf<String?>(null) }
    var madeKey by remember { mutableStateOf<KeyRecord?>(null) }
    val scope = rememberCoroutineScope()
    val usernameError = if (username.isEmpty()) null else hostFieldError(username)
    val valid = validHost(label, addresses, username) && (newKey || keys.any { it.id == keyId })

    installKeyId?.let { id ->
        val key = keys.find { it.id == id } ?: madeKey?.takeIf { it.id == id }
        if (key != null) {
            PairInstallKeyScreen(label.trim(), key.openssh, key.fingerprint, done = saved, trusted = false)
            return
        }
    }

    fun host(key: String?) = Host(
        HostRecord(previous?.id ?: 0, label.trim(), username, key, showInInbox, transport, sleeps, previous?.moshFailedUntil ?: 0),
        addresses.map { HostEndpoint(it.hostname, it.port.toInt()) },
    )
    fun submit() {
        if (!valid || busy || working) return
        if (!newKey) {
            save(host(keyId))
            saved()
            return
        }
        working = true
        keyError = null
        scope.launch {
            val key = try {
                createKey(newKeyLabel(label), newKeyComment(deviceLabel))
            } catch (error: CancellationException) {
                throw error
            } catch (error: Exception) {
                keyError = newKeyErrorMessage(error)
                working = false
                return@launch
            }
            // The key is stored: from here on it is an ordinary choice, so a second Save never makes another.
            madeKey = key
            keyId = key.id
            newKey = false
            working = false
            save(host(key.id))
            installKeyId = key.id
        }
    }
    val scroll = rememberScrollState()
    Column(Modifier.fillMaxSize()) {
        TopBar(
            title = if (previous == null) "New connection" else "Edit connection", back = close, backIcon = Or2Icons.Close, backDescription = "Close",
            scrolled = scroll.scrolledUnder(),
            actions = { IconAction(Or2Icons.Check, "Save", ::submit, Modifier.testTag("host-form-save"), tint = Or2Colors.Accent, enabled = valid && !busy && !working) },
        )
        Column(
            Modifier.weight(1f).verticalScroll(scroll).padding(horizontal = Or2Dimens.Gutter).testTag("host-form"),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Spacer(Modifier.height(0.dp))
            Or2Field(label, { label = it }, label = "Name", placeholder = "My server", mono = false, tag = "host-label")
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                    Text("Addresses", style = Or2Type.Body, color = Or2Colors.Text)
                    Text(ADDRESS_ORDER_HINT, style = Or2Type.Secondary, color = Or2Colors.TextMuted, modifier = Modifier.testTag("host-address-hint"))
                }
                addresses.forEachIndexed { index, address ->
                    AddressRow(
                        index, address, count = addresses.size,
                        change = { updated -> addresses = addresses.toMutableList().also { it[index] = updated } },
                        up = { addresses = addresses.moved(index, -1) },
                        down = { addresses = addresses.moved(index, 1) },
                        remove = { addresses = addresses.filterIndexed { i, _ -> i != index } },
                    )
                }
                PillButton("Add address", { addresses = addresses + AddressDraft("", "22") }, Modifier.testTag("address-add").fillMaxWidth(),
                    icon = Or2Icons.Plus, enabled = addresses.size < Host.MAX_ADDRESSES)
            }
            Or2Field(username, { username = it }, label = "Username", placeholder = "your-username",
                errorText = usernameError, tag = "host-username")
            Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Text("SSH key", style = Or2Type.Body, color = Or2Colors.Text)
                // A fresh form is calm: the hint is muted, not an error, until a key is chosen.
                if (!newKey && keys.none { it.id == keyId }) Text("Choose a key", style = Or2Type.Secondary, color = Or2Colors.TextMuted)
                KeyPicker(
                    keys, selectedKeyId = keyId, newSelected = newKey, enabled = !working,
                    choose = { keyId = it; newKey = false }, chooseNew = { newKey = true }, tagPrefix = "host-key",
                )
                if (newKey) {
                    Text(
                        "Made when you save. Its public key, to add to authorized_keys on the host, is shown next.",
                        style = Or2Type.Secondary, color = Or2Colors.TextMuted, modifier = Modifier.testTag("host-key-new-note"),
                    )
                }
            }
            Column(verticalArrangement = Arrangement.spacedBy(Or2Dimens.SectionHeaderGap)) {
                Text("Transport", style = Or2Type.Body, color = Or2Colors.Text)
                Segmented(
                    TransportChoices.map(::transportLabel), TransportChoices.indexOf(transport), { transport = TransportChoices[it] },
                    Modifier.testTag("host-transport"), tagPrefix = "host-transport",
                )
                Text(transportExplanation(transport), style = Or2Type.Secondary, color = Or2Colors.TextMuted,
                    modifier = Modifier.testTag("host-transport-note"))
            }
            GroupCard {
                ListRow(
                    "Show agents in the inbox", onClick = { showInInbox = !showInInbox },
                    trailing = { Or2Toggle(showInInbox, { showInInbox = it }, Modifier.testTag("host-inbox")) },
                )
                GroupDivider(inset = Or2Dimens.Gutter)
                ListRow(
                    "Host sleeps when idle", onClick = { sleeps = !sleeps },
                    trailing = { Or2Toggle(sleeps, { sleeps = it }, Modifier.testTag("host-sleeps")) },
                )
            }
            Text(SLEEPS_EXPLANATION, style = Or2Type.Secondary, color = Or2Colors.TextMuted, modifier = Modifier.testTag("host-sleeps-note"))
            if (previous != null) {
                Text("Changing any address or port clears previous host-key trust.", style = Or2Type.Secondary, color = Or2Colors.TextMuted)
            }
            keyError?.let { Text(it, style = Or2Type.Secondary, color = Or2Colors.Danger, modifier = Modifier.testTag("host-form-error")) }
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                PrimaryButton(if (working) "Working…" else "Save", ::submit, Modifier.testTag("host-form-primary"), enabled = valid && !busy && !working)
                Text(
                    "Private keys stay encrypted in hardware-backed storage on this device. Connecting always needs your biometric.",
                    style = Or2Type.Secondary, color = Or2Colors.TextMuted, modifier = Modifier.fillMaxWidth().padding(horizontal = 6.dp),
                )
            }
            Spacer(Modifier.height(24.dp))
            BottomInsetSpacer()
        }
    }
}

/** An address list as saved state: hostname and port strings, flattened. */
private val AddressDraftsSaver = listSaver<List<AddressDraft>, String>(
    save = { drafts -> drafts.flatMap { listOf(it.hostname, it.port) } },
    restore = { flat -> flat.chunked(2).map { AddressDraft(it[0], it[1]) } },
)

/** One address: hostname and port side by side, with its order and remove controls below. */
@Composable
private fun AddressRow(
    index: Int, address: AddressDraft, count: Int, change: (AddressDraft) -> Unit,
    up: () -> Unit, down: () -> Unit, remove: () -> Unit,
) {
    val hostnameError = if (address.hostname.isEmpty()) null else hostFieldError(address.hostname)
    Column(Modifier.testTag("address:$index"), verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.Top) {
            Or2Field(address.hostname, { change(address.copy(hostname = it)) }, Modifier.weight(1f), tag = "address-hostname:$index",
                label = "Host", placeholder = "192.0.2.10", errorText = hostnameError,
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, autoCorrectEnabled = false))
            Or2Field(address.port, { change(address.copy(port = it)) }, Modifier.width(80.dp), tag = "address-port:$index",
                label = "Port", placeholder = "22", errorText = if (portError(address.port) != null) "1-65535" else null,
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number))
        }
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End, verticalAlignment = Alignment.CenterVertically) {
            Text("Address ${index + 1}", style = Or2Type.Secondary, color = Or2Colors.TextMuted, modifier = Modifier.weight(1f))
            IconAction(Or2Icons.ArrowUp, "Move address ${index + 1} up", up, Modifier.testTag("address-up:$index"), tint = Or2Colors.TextMuted, enabled = index > 0)
            IconAction(Or2Icons.ArrowDown, "Move address ${index + 1} down", down, Modifier.testTag("address-down:$index"), tint = Or2Colors.TextMuted, enabled = index < count - 1)
            IconAction(Or2Icons.Trash, "Remove address ${index + 1}", remove, Modifier.testTag("address-remove:$index"), tint = Or2Colors.TextMuted, enabled = count > 1)
        }
    }
}
