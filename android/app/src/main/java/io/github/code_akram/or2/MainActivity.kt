package io.github.code_akram.or2

import android.net.Uri
import android.os.Bundle
import androidx.activity.compose.BackHandler
import androidx.activity.compose.LocalActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.displayCutout
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.layout.union
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.unit.dp
import androidx.core.view.WindowCompat
import androidx.fragment.app.FragmentActivity
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.lifecycleScope
import io.github.code_akram.or2.app.AppViewModel
import io.github.code_akram.or2.app.Or2Application
import io.github.code_akram.or2.data.HostRecord
import io.github.code_akram.or2.ffi.ClientKeyMaterial
import io.github.code_akram.or2.ffi.ConnectException
import io.github.code_akram.or2.ffi.KeyException
import io.github.code_akram.or2.ffi.SessionException
import io.github.code_akram.or2.ffi.generateEd25519Key
import io.github.code_akram.or2.hosts.HostsScreen
import io.github.code_akram.or2.keys.KeysScreen
import io.github.code_akram.or2.keys.VaultException
import io.github.code_akram.or2.keys.authenticateCipher
import io.github.code_akram.or2.keys.encryptKey
import io.github.code_akram.or2.keys.importAndWipe
import io.github.code_akram.or2.keys.keyErrorMessage
import io.github.code_akram.or2.keys.readPrivateKey
import io.github.code_akram.or2.keys.vaultErrorMessage
import io.github.code_akram.or2.session.SessionHolder
import io.github.code_akram.or2.session.SessionScreen
import io.github.code_akram.or2.session.connectErrorMessage
import io.github.code_akram.or2.session.sessionErrorMessage
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.security.GeneralSecurityException
import java.util.UUID
import android.graphics.Color as AndroidColor

class MainActivity : FragmentActivity() {
    private val app get() = application as Or2Application
    private lateinit var model: AppViewModel
    private var busy by mutableStateOf(false)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge(
            statusBarStyle = SystemBarStyle.light(AndroidColor.TRANSPARENT, AndroidColor.TRANSPARENT),
            navigationBarStyle = SystemBarStyle.light(AndroidColor.TRANSPARENT, AndroidColor.TRANSPARENT),
        )
        model = ViewModelProvider(this, object : ViewModelProvider.Factory {
            @Suppress("UNCHECKED_CAST")
            override fun <T : ViewModel> create(modelClass: Class<T>): T = AppViewModel(app.database.dao(), app.vault::delete) as T
        })[AppViewModel::class.java]
        setContent {
            val hosts by model.hosts.collectAsStateWithLifecycle()
            val keys by model.keys.collectAsStateWithLifecycle()
            val message by model.message.collectAsStateWithLifecycle()
            val active by app.sessions.active.collectAsStateWithLifecycle()
            var tab by rememberSaveable { mutableStateOf("Hosts") }
            AppScaffold(app.sessions, tab, { tab = it }) {
                message?.let {
                    Text(it, color = MaterialTheme.colorScheme.error)
                    TextButton(onClick = { model.message(null) }) { Text("Dismiss") }
                }
                if (busy) Text("Waiting for authentication or operation…")
                when (tab) {
                    "Hosts" -> HostsScreen(hosts, keys, busy, { host, previous ->
                        if (active?.host?.id == host.id) app.sessions.disconnect()
                        model.saveHost(host, previous)
                    }, { host ->
                        if (active?.host?.id == host.id) app.sessions.disconnect()
                        model.deleteHost(host)
                    }) { host ->
                        tab = "Session"
                        connectHost(host)
                    }
                    "Keys" -> KeysScreen(keys, busy, { label, comment ->
                        saveKey(label) { generateEd25519Key(comment) }
                    }, ::importKey, model::deleteKey)
                    else -> SessionScreen(app.sessions, busy, { current, prompt -> operation { app.sessions.approve(current, prompt) } },
                        { current -> operation { app.sessions.reject(current) } })
                }
            }
        }
    }

    private fun operation(block: suspend () -> Unit) {
        if (busy) return
        busy = true
        model.message(null)
        lifecycleScope.launch {
            try { block() } catch (error: CancellationException) { throw error }
            catch (error: Exception) {
                model.message(when (error) {
                    is KeyException -> keyErrorMessage(error)
                    is ConnectException -> connectErrorMessage(error)
                    is SessionException -> sessionErrorMessage(error)
                    is VaultException, is GeneralSecurityException -> vaultErrorMessage(error)
                    else -> "Operation failed. No host-key approval was sent. Retry or reconnect if the prompt expired."
                })
            } finally { busy = false }
        }
    }

    private fun importKey(uri: Uri, label: String, passphrase: String?) = saveKey(label) {
        val bytes = contentResolver.openInputStream(uri)?.let(::readPrivateKey)
            ?: throw VaultException("The selected file cannot be opened. Choose it again.")
        importAndWipe(bytes, passphrase)
    }

    private fun saveKey(label: String, produce: () -> ClientKeyMaterial) = operation {
        // The entire material lifetime stays inside this worker block: cancellation at a
        // withContext return must never strand plaintext returned by native generation/import.
        withContext(Dispatchers.IO) {
            val id = UUID.randomUUID().toString()
            var saved = false
            try {
                val record = encryptKey(id, label.trim(), produce()) {
                    val cipher = app.vault.createCipher(id)
                    withContext(Dispatchers.Main) { authenticateCipher(this@MainActivity, cipher, "Save SSH key") }
                }
                ensureActive()
                withContext(NonCancellable) {
                    app.database.dao().insertKey(record)
                    saved = true
                }
            } finally {
                if (!saved) app.vault.delete(id)
            }
        }
        model.message("Key saved. Copy or share its public key for manual installation.")
    }

    private fun connectHost(host: HostRecord) = operation {
        withContext(Dispatchers.IO) {
            val key = host.keyId?.let { app.database.dao().key(it) }
                ?: throw VaultException("Select a stored key for this host first.")
            val cipher = app.vault.decryptCipher(key.id, key.iv)
            val unlocked = withContext(Dispatchers.Main) { authenticateCipher(this@MainActivity, cipher, "Unlock SSH key") }
            val bytes = unlocked.doFinal(key.ciphertext)
            try {
                withContext(Dispatchers.Main) { app.sessions.connect(host, bytes) }
            } finally {
                bytes.fill(0)
            }
        }
    }
}

/** Shared navigation chrome; terminal sessions use only system-bar/cutout insets, not form padding. */
@Composable
fun AppScaffold(holder: SessionHolder, tab: String, selectTab: (String) -> Unit, content: @Composable () -> Unit) {
    val current by holder.active.collectAsStateWithLifecycle()
    val hasConnected = key(current) { current?.hasConnected?.collectAsStateWithLifecycle()?.value == true }
    val terminalVisible = tab == "Session" && hasConnected
    val activity = LocalActivity.current
    val view = LocalView.current
    SideEffect {
        activity?.let {
            val controller = WindowCompat.getInsetsController(it.window, view)
            controller.isAppearanceLightStatusBars = !terminalVisible
            controller.isAppearanceLightNavigationBars = !terminalVisible
        }
    }
    BackHandler(enabled = tab == "Session") { selectTab("Hosts") }
    MaterialTheme(colorScheme = if (terminalVisible) darkColorScheme(background = Color.Black, surface = Color(0xff101010)) else lightColorScheme()) {
        Surface(modifier = Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
            Column(
                modifier = Modifier.fillMaxSize()
                    .windowInsetsPadding(WindowInsets.systemBars.union(WindowInsets.displayCutout))
                    .then(if (tab == "Session") Modifier else Modifier.imePadding())
                    .then(if (terminalVisible) Modifier else Modifier.padding(16.dp)),
                verticalArrangement = Arrangement.spacedBy(if (terminalVisible) 0.dp else 12.dp),
            ) {
                if (!terminalVisible) {
                    Text("or2", style = MaterialTheme.typography.headlineMedium)
                    Row {
                        listOf("Hosts", "Keys", "Session").forEach { name ->
                            TextButton(onClick = { selectTab(name) }) { Text(if (tab == name) "• $name" else name) }
                        }
                    }
                }
                content()
            }
        }
    }
}
