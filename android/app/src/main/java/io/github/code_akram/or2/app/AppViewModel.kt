package io.github.code_akram.or2.app

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import io.github.code_akram.or2.data.AppDao
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.hosts.AddressDraft
import io.github.code_akram.or2.hosts.validHost
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.onEach
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch

class AppViewModel(private val dao: AppDao, private val deleteVaultKey: (String) -> Unit) : ViewModel() {
    private val hostsRead = MutableStateFlow(false)
    private val keysRead = MutableStateFlow(false)
    val hosts = dao.hosts().onEach { hostsRead.value = true }.stateIn(viewModelScope, SharingStarted.WhileSubscribed(5000), emptyList())
    val keys = dao.keys().onEach { keysRead.value = true }.stateIn(viewModelScope, SharingStarted.WhileSubscribed(5000), emptyList())

    /**
     * False until the stored hosts and keys have both been read once. Until then `hosts` and `keys`
     * are empty placeholders, so a restored edit form or host page must wait instead of treating
     * its host as missing (and saving an edit as a new host).
     */
    val loaded = combine(hostsRead, keysRead) { hostsDone, keysDone -> hostsDone && keysDone }
        .stateIn(viewModelScope, SharingStarted.Eagerly, false)
    private val mutableMessage = MutableStateFlow<String?>(null)
    val message = mutableMessage.asStateFlow()
    fun message(text: String?) { mutableMessage.value = text }

    /**
     * [afterSave] runs once the host is written, never when the write fails (e.g. to end a
     * connection the edit made stale: a failed edit changed nothing, so the connection stays).
     */
    fun saveHost(host: Host, previous: Host?, afterSave: () -> Unit = {}) {
        val normalized = host.copy(
            record = host.record.copy(username = host.username.trim()),
            addresses = host.addresses.map { it.copy(hostname = it.hostname.trim()) },
        )
        if (!validHost(normalized.label, normalized.addresses.map(AddressDraft::of), normalized.username)) {
            message("Host not saved. Enter a label, 1 to ${Host.MAX_ADDRESSES} addresses with valid ports, and a hostname and username without internal whitespace or control characters.")
            return
        }
        action {
            dao.saveHost(normalized, previous)
            afterSave()
        }
    }

    /** [afterDelete] runs only once the host is gone from storage. */
    fun deleteHost(host: Host, afterDelete: () -> Unit = {}) = action {
        dao.deleteHost(host.id)
        afterDelete()
    }
    fun deleteKey(id: String) = action {
        dao.deleteKey(id) // Clear host references transactionally before destroying the vault entry.
        deleteVaultKey(id)
    }

    private fun action(block: suspend () -> Unit) {
        viewModelScope.launch {
            try { block() } catch (error: CancellationException) { throw error }
            catch (_: Exception) { message("Storage operation failed. No connection was authorized. Retry.") }
        }
    }
}
