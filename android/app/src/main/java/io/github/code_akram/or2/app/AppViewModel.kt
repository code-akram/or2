package io.github.code_akram.or2.app

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import io.github.code_akram.or2.data.AppDao
import io.github.code_akram.or2.data.HostRecord
import io.github.code_akram.or2.hosts.validHost
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch

class AppViewModel(private val dao: AppDao, private val deleteVaultKey: (String) -> Unit) : ViewModel() {
    val hosts = dao.hosts().stateIn(viewModelScope, SharingStarted.WhileSubscribed(5000), emptyList())
    val keys = dao.keys().stateIn(viewModelScope, SharingStarted.WhileSubscribed(5000), emptyList())
    private val mutableMessage = MutableStateFlow<String?>(null)
    val message = mutableMessage.asStateFlow()
    fun message(text: String?) { mutableMessage.value = text }

    fun saveHost(host: HostRecord, previous: HostRecord?) {
        val normalized = host.copy(hostname = host.hostname.trim(), username = host.username.trim())
        if (!validHost(normalized.label, normalized.hostname, normalized.port.toString(), normalized.username)) {
            message("Host not saved. Enter a label, a valid port, and a hostname and username without internal whitespace or control characters.")
            return
        }
        action { dao.saveHost(normalized, previous) }
    }

    fun deleteHost(host: HostRecord) = action { dao.deleteHost(host.id) }
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
