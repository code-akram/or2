package io.github.code_akram.or2.app

import android.Manifest
import android.app.Application
import android.content.ClipData
import android.content.ClipboardManager
import android.content.pm.ApplicationInfo
import android.content.pm.PackageManager
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import android.os.SystemClock
import android.util.Log
import androidx.room.Room
import io.github.code_akram.or2.connection.HostConnections
import io.github.code_akram.or2.connection.HostConnector
import io.github.code_akram.or2.connection.MoshServerLedger
import io.github.code_akram.or2.connection.TIMING_TAG
import io.github.code_akram.or2.connection.Timing
import io.github.code_akram.or2.data.AppDatabase
import io.github.code_akram.or2.data.MIGRATION_1_2
import io.github.code_akram.or2.data.MIGRATION_2_3
import io.github.code_akram.or2.data.MIGRATION_3_4
import io.github.code_akram.or2.keys.BiometricVault
import io.github.code_akram.or2.notify.AgentAlertSettings
import io.github.code_akram.or2.notify.AgentAlerts
import io.github.code_akram.or2.notify.AgentNotifications
import io.github.code_akram.or2.notify.AgentOpenRequests
import io.github.code_akram.or2.notify.AgentReplies
import io.github.code_akram.or2.notify.EnableReplyRequests
import io.github.code_akram.or2.notify.ReplyNonces
import io.github.code_akram.or2.ffi.networkChanged
import io.github.code_akram.or2.service.ConnectionService
import io.github.code_akram.or2.service.NetworkChanges
import io.github.code_akram.or2.service.ServiceSnapshot
import io.github.code_akram.or2.service.ServiceStarter
import io.github.code_akram.or2.service.serviceSnapshots
import io.github.code_akram.or2.terminal.HostClipboard
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.shareIn
import kotlinx.coroutines.launch

class Or2Application : Application() {
    val database by lazy { Room.databaseBuilder(this, AppDatabase::class.java, "or2.db")
        // Never destructive: key records are bound to Keystore entries that cannot be recreated.
        .addMigrations(MIGRATION_1_2, MIGRATION_2_3, MIGRATION_3_4).build() }
    val vault by lazy { BiometricVault(this) }

    /** App-private settings: the one-time prompts and the last terminal. */
    val prefs: PrefStore by lazy { SharedPrefsStore(this) }
    val reattach by lazy { ReattachMemory(prefs) }

    /** The mosh servers this app started, so one orphaned by process death is stopped at the next connect. */
    val moshServers by lazy { MoshServerLedger(prefs) }
    /** `POST_NOTIFICATIONS`, offered in context only (never on connect); the service runs without it. */
    val notifications by lazy {
        NotificationPermission(prefs) { checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED }
    }

    /** The `Agent notifications` switch in Settings (on by default). */
    val agentAlertSettings by lazy { AgentAlertSettings(prefs) }

    /** Agent notifications: one per Blocked or Done edge a live herdr watch sees, none for the pane on screen. */
    val agentAlerts by lazy {
        AgentAlerts(AgentNotifications(this, prefs), ReplyNonces(prefs)) { agentAlertSettings.enabled.value }
            .also { alerts -> alerts.enableReply = { hostId, agent -> connections.enableReplyFor(hostId, agent) } }
    }

    /** An Enable Reply waiting for its confirmation (an Inbox row's, a notification's), for the dialog. */
    val enableReplies = EnableReplyRequests()

    /**
     * A notification's Reply ([AgentReplyReceiver]): over the host's live connection only, on the application's own
     * scope (the broadcast does not wait for it) and the main dispatcher like the connections it uses.
     */
    val agentReplies by lazy {
        AgentReplies(
            CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate),
            admit = agentAlerts::admitReply,
            send = { key, agent, text -> connections.replyToPane(key.hostId, key.session, key.paneId, agent, text) },
            post = agentAlerts::replied,
        )
    }

    /** A notification's tap, from the activity's intent to the UI that opens the pane. */
    val agentOpens = AgentOpenRequests()

    /** The battery exemption: the last step of adding a host, once; Home's card after a decline. */
    val battery by lazy { BatteryPrompt(prefs, isExempt = ::isBatteryExempt) }

    /**
     * What the previous process left behind: whether it died with sessions open (a cold launcher start
     * then resumes the remembered terminal). Created before anything of this process writes to it.
     */
    val sessionMarker by lazy { SessionMarker(prefs) }

    /** Timing markers for the critical paths (logcat tag `or2.timing`), recorded only in a debuggable build. */
    val timing by lazy {
        val debuggable = applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE != 0
        Timing(if (debuggable) { line -> Log.d(TIMING_TAG, line) } else null)
    }

    /** Device tests point this at a scripted host (`contract_probe_host`) and restore it; production never sets it. */
    @Volatile
    var connectorOverride: HostConnector? = null

    /** The process's one set of connections; [ConnectionService] keeps the process alive while any is open. */
    val connections: HostConnections by lazy {
        HostConnections({ request, listener -> (connectorOverride ?: HostConnector.Native).connect(request, listener) }, database.dao(),
            moshServers = moshServers, timing = timing)
            .also {
                it.userClose = reattach
                it.herdrObserver = agentAlerts
                it.clipboardWrite = hostClipboard::offer
            }
    }

    /** The user's settings (the Settings screen). */
    val settings by lazy { AppSettings(prefs) }

    /** Clipboard writes from hosts (OSC 52) into the Android clipboard, labelled `or2`. */
    val hostClipboard by lazy {
        val main = Handler(Looper.getMainLooper())
        HostClipboard(
            enabled = { settings.copyFromHost.value },
            now = SystemClock::uptimeMillis,
            schedule = { delay, action -> main.postDelayed(action, delay) },
        ) { text ->
            runCatching { getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText("or2", text)) }
        }
    }

    /**
     * The one debouncer of `network_changed()`: the service's network callbacks and every return to
     * the foreground feed it, so a burst of events (and a return right after a handover) is one roam.
     * Calling it with nothing live is free: the registry of live sessions is empty.
     */
    val networkChanges by lazy {
        NetworkChanges(CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)) { networkChanged() }
    }

    /** The process's own scope on main: it lives as long as the process (an `Application` is never destroyed). */
    private val processScope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    /**
     * What is open, for the foreground service and its starter: [connections]' snapshots collected once for the
     * process and shared, the latest replayed to each new collector (a service that starts again gets it at once).
     * The upstream runs in [processScope]; a collector (the service's) that is cancelled stops only itself.
     */
    val serviceSnapshots: Flow<ServiceSnapshot> by lazy {
        connections.serviceSnapshots().shareIn(processScope, SharingStarted.Eagerly, replay = 1)
    }

    private var starter: ServiceStarter? = null

    /** Starts the foreground service whenever a host or session opens and it is not running. Idempotent. */
    fun watchConnections() {
        if (starter != null) return
        val created = ServiceStarter { ConnectionService.start(this) }
        starter = created
        // Read what the previous process left before this one starts writing.
        val marker = sessionMarker
        processScope.launch {
            serviceSnapshots.collect { snapshot ->
                created.onSnapshot(snapshot)
                marker.onOpenSessions(snapshot.sessions > 0)
            }
        }
    }

    /**
     * Starts the service again when connections are open and it is not running: it was destroyed
     * from outside, or a start was refused while the app was in the background. Call on main when
     * the service is destroyed and when the app comes to the foreground.
     */
    fun reviveService() {
        starter?.recheck()
    }

    fun isBatteryExempt(): Boolean = getSystemService(PowerManager::class.java).isIgnoringBatteryOptimizations(packageName)
}
