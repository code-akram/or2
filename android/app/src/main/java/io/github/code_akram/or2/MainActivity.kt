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
import io.github.code_akram.or2.app.BatteryStage
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
    private lateinit var pairModel: PairViewModel
    private var busy by mutableStateOf(false)

    /**
     * The hosts of a connect that waits for the `POST_NOTIFICATIONS` dialog, by id. It is saved
     * with the instance state: the dialog's result goes to whichever activity instance exists when
     * it returns (rotation, a theme change or process death while it is up), and the tap must not
     * be lost with the old one. [busy] covers the wait, so a second tap cannot start a connect in
     * parallel and a Resume waiting on the connect does not take the quiet moment for its end.
     */
    private var pendingConnect: LongArray? = null
    private val notificationPermission = registerForActivityResult(ActivityResultContracts.RequestPermission()) {
        // Granted or not, connecting goes on: the service runs without its notification being visible.
        val ids = pendingConnect ?: return@registerForActivityResult
        pendingConnect = null
        lifecycleScope.launch {
            val hosts = ids.toList().mapNotNull { app.database.dao().host(it) }
            // Back to back with no suspension between: busy never reads false in between.
            busy = false
            if (hosts.isNotEmpty()) connectAfterNotifications(hosts)
        }
    }

    /**
     * The hosts of a connect that waits for the battery-optimisation explanation (and the system's own
     * request after it), by id, saved like [pendingConnect]: the dialog is up in the foreground before
     * the first unlock, and a connect must survive the activity being recreated meanwhile.
     */
    private var pendingBattery: LongArray? = null
    private val batteryExemption = registerForActivityResult(ActivityResultContracts.StartActivityForResult()) {
        // The system's dialog is closed; whatever it answered, the exemption is read afresh and connecting goes on.
        if (app.isBatteryExempt()) app.battery.refresh() else app.battery.declined()
        continueAfterBattery()
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        pendingConnect = savedInstanceState?.getLongArray(PENDING_CONNECT)
        pendingBattery = savedInstanceState?.getLongArray(PENDING_BATTERY)
        if (pendingConnect != null || pendingBattery != null) busy = true
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
        pairModel = ViewModelProvider(this, object : ViewModelProvider.Factory {
            @Suppress("UNCHECKED_CAST")
            override fun <T : ViewModel> create(modelClass: Class<T>): T = PairViewModel(app.database.dao(), ::describeKeyError) as T
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
            answerBatteryExplanation = ::answerBatteryExplanation,
            takeColdResume = app.sessionMarker::takeColdResume,
            pair = pairModel.flow,
            deviceLabel = Build.MODEL.takeIf { it.isNotBlank() } ?: "Android phone",
            generatePairKey = { label, comment -> createKey(label) { generateEd25519Key(comment) } },
        )
        setContent {
            val hosts by model.hosts.collectAsStateWithLifecycle()
            val keys by model.keys.collectAsStateWithLifecycle()
            val message by model.message.collectAsStateWithLifecycle()
            val loaded by model.loaded.collectAsStateWithLifecycle()
            Or2App(hosts, keys, message, busy, app.connections, actions, loaded)
        }
        // A connect that waited on the battery flow: the explanation is memory-only, so a restored
        // process has none on screen. Put it back (or carry on), or `busy` would never end.
        if (pendingBattery != null) {
            when (app.battery.restoreStage()) {
                BatteryStage.EXPLANATION, BatteryStage.SYSTEM_REQUEST -> Unit // The answer, or the launcher's result, carries on.
                BatteryStage.PROCEED -> continueAfterBattery()
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
     * by native generation/import. Used by the keys screen and by Easy pair's "new key".
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

    /** What the pairing screen says when its new key could not be made (the biometric was cancelled, the vault refused). */
    private fun describeKeyError(error: Throwable): String = when (error) {
        is KeyException -> keyErrorMessage(error)
        is VaultException, is GeneralSecurityException -> vaultErrorMessage(error)
        else -> "The key could not be created. Try again."
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

    /** The first connection asks for `POST_NOTIFICATIONS` (Android 13+), once; then connects either way. */
    private fun connect(hosts: List<Host>) {
        if (busy) return
        val granted = checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED
        if (app.notificationPolicy.shouldAsk(Build.VERSION.SDK_INT, granted)) {
            app.notificationPolicy.markAsked()
            pendingConnect = hosts.map { it.id }.toLongArray()
            busy = true
            notificationPermission.launch(Manifest.permission.POST_NOTIFICATIONS)
        } else {
            connectAfterNotifications(hosts)
        }
    }

    /**
     * The battery-optimisation exemption is asked for up front, once: before the first unlock, in the
     * foreground (not as a modal over a terminal on a later return). The explanation is Compose's
     * (`battery.explaining`); "Allow" goes on to the system's own request, "Not now" to the connect.
     */
    private fun connectAfterNotifications(hosts: List<Host>) {
        if (app.battery.shouldExplain()) {
            pendingBattery = hosts.map { it.id }.toLongArray()
            busy = true
            app.battery.explain()
        } else {
            startConnect(hosts)
        }
    }

    private fun answerBatteryExplanation(allow: Boolean) {
        if (!app.battery.explaining.value) return
        app.battery.explained()
        if (allow && launchBatteryRequest()) return // The result callback carries on.
        app.battery.declined()
        continueAfterBattery()
    }

    /** Opens the system's request; false when this device has no such screen. */
    @SuppressLint("BatteryLife")
    private fun launchBatteryRequest(): Boolean = try {
        batteryExemption.launch(Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:$packageName")))
        true
    } catch (_: ActivityNotFoundException) {
        false
    }

    private fun continueAfterBattery() {
        val ids = pendingBattery ?: return
        pendingBattery = null
        lifecycleScope.launch {
            val hosts = ids.toList().mapNotNull { app.database.dao().host(it) }
            // Back to back with no suspension between: busy never reads false in between.
            busy = false
            if (hosts.isNotEmpty()) startConnect(hosts)
        }
    }

    override fun onStart() {
        super.onStart()
        // Connections without a service (it was stopped from outside, or a start was refused in the background): back to the foreground is the moment it may start.
        app.reviveService()
        // Whatever the network did while the app was away is settled by one roam (debounced with any
        // callback event that arrives with it).
        app.networkChanges.foregrounded()
        // The exemption may have been given (or taken away) in Settings while the app was away.
        app.battery.refresh()
    }

    override fun onSaveInstanceState(outState: Bundle) {
        super.onSaveInstanceState(outState)
        pendingConnect?.let { outState.putLongArray(PENDING_CONNECT, it) }
        pendingBattery?.let { outState.putLongArray(PENDING_BATTERY, it) }
    }

    private fun startConnect(hosts: List<Host>) = operation { connectGrouped(hosts, app.connections, biometricUnlocker) }

    private companion object {
        const val PENDING_CONNECT = "pending_connect"
        const val PENDING_BATTERY = "pending_battery"
    }

    /**
     * The system's own "let this app ignore battery optimisations?" dialog, from Home's card (the
     * explanation was shown once, before the first connection). Play Store policy restricts this request;
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
