package io.github.code_akram.or2

import android.Manifest
import android.annotation.SuppressLint
import android.content.ActivityNotFoundException
import android.content.Intent
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
import io.github.code_akram.or2.app.NotificationGrant
import io.github.code_akram.or2.app.NotificationUse
import io.github.code_akram.or2.app.Or2App
import io.github.code_akram.or2.app.Or2Application
import io.github.code_akram.or2.connection.KeyUnlocker
import io.github.code_akram.or2.connection.MissingKeyException
import io.github.code_akram.or2.connection.connectGrouped
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.KeyRecord
import io.github.code_akram.or2.ffi.ClientKeyMaterial
import io.github.code_akram.or2.ffi.HostConnectException
import io.github.code_akram.or2.ffi.HostException
import io.github.code_akram.or2.ffi.KeyException
import io.github.code_akram.or2.ffi.generateEd25519Key
import io.github.code_akram.or2.keys.VaultException
import io.github.code_akram.or2.pair.PairViewModel
import io.github.code_akram.or2.keys.authenticateCipher
import io.github.code_akram.or2.keys.encryptKey
import io.github.code_akram.or2.keys.importAndWipe
import io.github.code_akram.or2.keys.keyErrorMessage
import io.github.code_akram.or2.keys.newKeyErrorMessage
import io.github.code_akram.or2.keys.readPrivateKey
import io.github.code_akram.or2.keys.vaultErrorMessage
import io.github.code_akram.or2.notify.AgentNotifications
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
    private lateinit var pairModel: PairViewModel
    private var busy by mutableStateOf(false)

    /**
     * Android's `POST_NOTIFICATIONS` dialog, opened only from an in-context offer (Home's "Show connection
     * notification"), never on connect. Its result goes to whichever activity instance exists when it returns, and
     * every offer re-reads the permission.
     */
    private val notificationRequest = registerForActivityResult(ActivityResultContracts.RequestPermission()) {
        app.notifications.refresh()
    }

    /**
     * The system's battery-optimisation request after "Allow" on the battery step (the last step of adding a host).
     * Whatever it answered, the step is done: the paired host connects, or the screen goes back to where the host was
     * added from. Registered in every instance, so a result that arrives after a recreation still ends the step.
     */
    private val batteryExemption = registerForActivityResult(ActivityResultContracts.StartActivityForResult()) {
        app.battery.requestClosed()
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        app.watchConnections()
        // A notification's tap that started (or restarted) the activity; a recreation does not repeat it.
        if (savedInstanceState == null) openAgentFrom(intent)
        enableEdgeToEdge(
            // Dark only: transparent bars with light icons over the app's own background.
            statusBarStyle = SystemBarStyle.dark(AndroidColor.TRANSPARENT),
            navigationBarStyle = SystemBarStyle.dark(AndroidColor.TRANSPARENT),
        )
        model = ViewModelProvider(this, object : ViewModelProvider.Factory {
            @Suppress("UNCHECKED_CAST")
            override fun <T : ViewModel> create(modelClass: Class<T>): T = AppViewModel(app.database.dao(), app.vault::delete) as T
        })[AppViewModel::class.java]
        pairModel = ViewModelProvider(this, object : ViewModelProvider.Factory {
            @Suppress("UNCHECKED_CAST")
            override fun <T : ViewModel> create(modelClass: Class<T>): T = PairViewModel(app.database.dao(), ::newKeyErrorMessage) as T
        })[PairViewModel::class.java]
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
            answerKeepAlive = ::answerKeepAlive,
            notifications = app.notifications.offer(NotificationUse.AGENT_ALERTS),
            allowNotifications = ::allowNotifications,
            takeColdResume = app.sessionMarker::takeColdResume,
            pair = pairModel.flow,
            deviceLabel = Build.MODEL.takeIf { it.isNotBlank() } ?: "Android phone",
            createKey = { label, comment -> createKey(label) { generateEd25519Key(comment) } },
            agentAlerts = app.agentAlertSettings,
            setAgentAlerts = ::setAgentAlerts,
            agentOpens = app.agentOpens,
            onScreen = app.agentAlerts::screenChanged,
        )
        setContent {
            val hosts by model.hosts.collectAsStateWithLifecycle()
            val keys by model.keys.collectAsStateWithLifecycle()
            val message by model.message.collectAsStateWithLifecycle()
            val loaded by model.loaded.collectAsStateWithLifecycle()
            Or2App(hosts, keys, message, busy, app.connections, actions, loaded)
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        openAgentFrom(intent)
    }

    /**
     * An agent notification's tap: its notification goes, and the UI opens the pane through `launchOpenAgent` (the
     * inbox tap's path), connecting the host first when it is not connected. Any other intent is ignored.
     */
    private fun openAgentFrom(intent: Intent?) {
        val pane = AgentNotifications.paneOf(intent, app.prefs) ?: return
        app.agentAlerts.opened(pane)
        app.agentOpens.request(pane)
    }

    /** The Settings switch. Turned on without the notification permission, it asks for it (in context). */
    private fun setAgentAlerts(on: Boolean) {
        app.agentAlertSettings.set(on)
        app.agentAlerts.enabledChanged()
        if (on && !app.notifications.isGranted()) allowNotifications()
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
        createKey(label, produce)
        model.message("Key saved. Copy or share its public key for manual installation.")
    }

    /**
     * Encrypts and stores a new key (one biometric prompt) and returns its record. The entire material lifetime
     * stays inside this worker block: cancellation at a withContext return must never strand plaintext returned
     * by native generation/import. Used by the keys screen and by **New key** (Easy pair's review, the host form).
     */
    private suspend fun createKey(label: String, produce: () -> ClientKeyMaterial): KeyRecord = withContext(Dispatchers.IO) {
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
            record
        } finally {
            if (!saved) app.vault.delete(id)
        }
    }

    /**
     * Edits that change a live connection's destination, login or key end it, after they are
     * saved; the inbox flag starts or stops its herdr watches; others leave it alone.
     */
    private fun saveHost(host: Host, previous: Host?) {
        model.saveHost(host, previous) {
            if (previous == null) return@saveHost
            app.connections.hostEdited(previous, host)
        }
    }

    /**
     * Every connect, the first one on a fresh install included, goes straight to the biometric unlock: no permission
     * or battery dialog comes first (the battery exemption is the last step of adding a host, and the notification
     * permission is offered in context).
     */
    private fun connect(hosts: List<Host>) = operation { connectGrouped(hosts, app.connections, biometricUnlocker) }

    /**
     * The battery step was answered. "Allow" opens the system's request (the step waits for its result); a device with
     * no such screen, like "Not now", ends the step at once and leaves Home's card.
     */
    private fun answerKeepAlive(allow: Boolean) {
        if (!app.battery.answer(allow)) return
        if (allow && !launchBatteryRequest()) app.battery.requestClosed()
    }

    /** Opens the system's request; false when this device has no such screen. */
    @SuppressLint("BatteryLife")
    private fun launchBatteryRequest(): Boolean = try {
        batteryExemption.launch(Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:$packageName")))
        true
    } catch (_: ActivityNotFoundException) {
        false
    }

    /**
     * "Allow" on a notification offer: Android's `POST_NOTIFICATIONS` dialog, or the app's notification settings once
     * Android no longer shows it (denied for good). The one entry point for every use ([NotificationUse]): Home's card
     * (connection status and agent alerts) and the Settings switch call it.
     */
    private fun allowNotifications() {
        val permission = app.notifications
        if (permission.isGranted()) return permission.refresh()
        when (permission.grant(shouldShowRequestPermissionRationale(Manifest.permission.POST_NOTIFICATIONS))) {
            NotificationGrant.REQUEST -> {
                permission.requested()
                notificationRequest.launch(Manifest.permission.POST_NOTIFICATIONS)
            }
            NotificationGrant.SETTINGS -> try {
                startActivity(Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, packageName))
            } catch (_: ActivityNotFoundException) {
                // No such screen on this device; the card stays until it is dismissed.
            }
        }
    }

    override fun onStart() {
        super.onStart()
        // Connections without a service (it was stopped from outside, or a start was refused in the background): back to the foreground is the moment it may start.
        app.reviveService()
        // Whatever the network did while the app was away is settled by one roam (debounced with any
        // callback event that arrives with it).
        app.networkChanges.foregrounded()
        // The exemption and the notification permission may have been given (or taken away) in Settings meanwhile.
        app.battery.refresh()
        app.notifications.refresh()
    }

    /**
     * The system's own "let this app ignore battery optimisations?" dialog, from Home's card (the
     * explanation was shown once, as the last step of adding a host). Play Store policy restricts this request;
     * or2 ships through F-Droid.
     */
    @SuppressLint("BatteryLife", "UseKtx")
    private fun requestBatteryExemption() {
        try {
            startActivity(Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:$packageName")))
        } catch (_: ActivityNotFoundException) {
            // No such screen on this device; the card stays until it is dismissed.
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
