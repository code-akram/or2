package io.github.code_akram.or2

import android.Manifest
import android.annotation.SuppressLint
import android.content.ActivityNotFoundException
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.fragment.app.FragmentActivity
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.lifecycleScope
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import io.github.code_akram.or2.app.AppActions
import io.github.code_akram.or2.app.AppViewModel
import io.github.code_akram.or2.app.Or2App
import io.github.code_akram.or2.app.Or2Application
import io.github.code_akram.or2.connection.KeyUnlocker
import io.github.code_akram.or2.connection.MissingKeyException
import io.github.code_akram.or2.connection.connectGrouped
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.ffi.ClientKeyMaterial
import io.github.code_akram.or2.ffi.HostConnectException
import io.github.code_akram.or2.ffi.HostException
import io.github.code_akram.or2.ffi.KeyException
import io.github.code_akram.or2.ffi.generateEd25519Key
import io.github.code_akram.or2.hosts.connectionAffectedBy
import io.github.code_akram.or2.keys.VaultException
import io.github.code_akram.or2.keys.authenticateCipher
import io.github.code_akram.or2.keys.encryptKey
import io.github.code_akram.or2.keys.importAndWipe
import io.github.code_akram.or2.keys.keyErrorMessage
import io.github.code_akram.or2.keys.readPrivateKey
import io.github.code_akram.or2.keys.vaultErrorMessage
import io.github.code_akram.or2.session.hostConnectErrorMessage
import io.github.code_akram.or2.session.hostErrorMessage
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
    private var afterNotificationAnswer: (() -> Unit)? = null
    private val notificationPermission = registerForActivityResult(ActivityResultContracts.RequestPermission()) {
        // Granted or not, connecting goes on: the service runs without its notification being visible.
        afterNotificationAnswer?.also { afterNotificationAnswer = null }?.invoke()
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        app.watchConnections()
        enableEdgeToEdge(
            // Dark only: transparent bars with light icons over the app's own background.
            statusBarStyle = SystemBarStyle.dark(AndroidColor.TRANSPARENT),
            navigationBarStyle = SystemBarStyle.dark(AndroidColor.TRANSPARENT),
        )
        model = ViewModelProvider(this, object : ViewModelProvider.Factory {
            @Suppress("UNCHECKED_CAST")
            override fun <T : ViewModel> create(modelClass: Class<T>): T = AppViewModel(app.database.dao(), app.vault::delete) as T
        })[AppViewModel::class.java]
        val actions = AppActions(
            saveHost = ::saveHost,
            // The connection ends only once the host is really gone from storage.
            deleteHost = { host -> model.deleteHost(host) { app.connections.release(host.id, closeTerminals = true) } },
            generateKey = { label, comment -> saveKey(label) { generateEd25519Key(comment) } },
            importKey = ::importKey,
            deleteKey = model::deleteKey,
            connect = ::connect,
            approve = { active, prompt -> operation { app.connections.approve(active, prompt) } },
            reject = { active -> operation { app.connections.reject(active) } },
            message = model::message,
            reattach = app.reattach,
            battery = app.battery,
            requestBatteryExemption = ::requestBatteryExemption,
        )
        setContent {
            val hosts by model.hosts.collectAsStateWithLifecycle()
            val keys by model.keys.collectAsStateWithLifecycle()
            val message by model.message.collectAsStateWithLifecycle()
            val loaded by model.loaded.collectAsStateWithLifecycle()
            Or2App(hosts, keys, message, busy, app.connections, actions, loaded)
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
                    is HostConnectException -> hostConnectErrorMessage(error)
                    is HostException -> hostErrorMessage(error)
                    is MissingKeyException -> error.message
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

    /**
     * Edits that change a live connection's destination, login or key end it, after they are
     * saved; the inbox flag starts or stops its herdr watches; others leave it alone.
     */
    private fun saveHost(host: Host, previous: Host?) {
        model.saveHost(host, previous) {
            if (previous == null) return@saveHost
            if (connectionAffectedBy(previous, host)) app.connections.release(host.id, closeTerminals = false)
            else app.connections.setWatching(host.id, host.showInInbox)
        }
    }

    /** The first connection asks for `POST_NOTIFICATIONS` (Android 13+), once; then connects either way. */
    private fun connect(hosts: List<Host>) {
        val granted = checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED
        if (app.notificationPolicy.shouldAsk(Build.VERSION.SDK_INT, granted)) {
            app.notificationPolicy.markAsked()
            afterNotificationAnswer = { startConnect(hosts) }
            notificationPermission.launch(Manifest.permission.POST_NOTIFICATIONS)
        } else {
            startConnect(hosts)
        }
    }

    private fun startConnect(hosts: List<Host>) = operation { connectGrouped(hosts, app.connections, biometricUnlocker) }

    /**
     * The system's own "let this app ignore battery optimisations?" dialog; shown only after our
     * explanation. Play Store policy restricts this request; or2 ships through F-Droid.
     */
    @SuppressLint("BatteryLife", "UseKtx")
    private fun requestBatteryExemption() {
        try {
            startActivity(Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:$packageName")))
        } catch (_: ActivityNotFoundException) {
            // No such screen on this device; the explanation was the one and only ask.
        }
    }

    /** One strong-biometric prompt per call; the decrypted array is wiped when the block ends. */
    private val biometricUnlocker = object : KeyUnlocker {
        override suspend fun <T> withKey(keyId: String, block: suspend (ByteArray) -> T): T {
            val key = app.database.dao().key(keyId)
                ?: throw VaultException("Select a stored key for this host first.")
            val cipher = withContext(Dispatchers.IO) { app.vault.decryptCipher(key.id, key.iv) }
            val unlocked = authenticateCipher(this@MainActivity, cipher, "Unlock SSH key")
            // Not cancellable: a cancelled return must never strand the plaintext before the try.
            val bytes = withContext(Dispatchers.IO + NonCancellable) { unlocked.doFinal(key.ciphertext) }
            try {
                return block(bytes)
            } finally {
                bytes.fill(0)
            }
        }
    }
}
