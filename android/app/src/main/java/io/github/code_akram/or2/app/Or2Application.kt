package io.github.code_akram.or2.app

import android.app.Application
import android.os.PowerManager
import androidx.room.Room
import io.github.code_akram.or2.connection.HostConnections
import io.github.code_akram.or2.connection.HostConnector
import io.github.code_akram.or2.data.AppDatabase
import io.github.code_akram.or2.data.MIGRATION_1_2
import io.github.code_akram.or2.data.MIGRATION_2_3
import io.github.code_akram.or2.keys.BiometricVault
import io.github.code_akram.or2.service.ConnectionService
import io.github.code_akram.or2.service.ServiceStarter
import io.github.code_akram.or2.service.serviceSnapshots
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch

class Or2Application : Application() {
    val database by lazy { Room.databaseBuilder(this, AppDatabase::class.java, "or2.db")
        // Never destructive: key records are bound to Keystore entries that cannot be recreated.
        .addMigrations(MIGRATION_1_2, MIGRATION_2_3).build() }
    val vault by lazy { BiometricVault(this) }

    /** App-private settings: the one-time prompts and the last terminal. */
    val prefs: PrefStore by lazy { SharedPrefsStore(this) }
    val reattach by lazy { ReattachMemory(prefs) }
    val notificationPolicy by lazy { NotificationPermissionPolicy(prefs) }
    val battery by lazy { BatteryPrompt(prefs, isExempt = ::isBatteryExempt) }

    /** Device tests point this at a scripted host (`contract_probe_host`) and restore it; production never sets it. */
    @Volatile
    var connectorOverride: HostConnector? = null

    /** The process's one set of connections; [ConnectionService] keeps the process alive while any is open. */
    val connections by lazy {
        HostConnections({ request, listener -> (connectorOverride ?: HostConnector.Native).connect(request, listener) }, database.dao())
            .also { it.userClose = reattach }
    }

    private var watching = false

    /** Starts the foreground service whenever a host or session opens and it is not running. Idempotent. */
    fun watchConnections() {
        if (watching) return
        watching = true
        val starter = ServiceStarter { ConnectionService.start(this) }
        CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate).launch {
            connections.serviceSnapshots().collect(starter::onSnapshot)
        }
    }

    fun isBatteryExempt(): Boolean = getSystemService(PowerManager::class.java).isIgnoringBatteryOptimizations(packageName)
}
