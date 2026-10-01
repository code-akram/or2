package io.github.code_akram.or2.service

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.ConnectivityManager
import android.net.Network
import android.os.IBinder
import android.os.Handler
import android.os.Looper
import io.github.code_akram.or2.MainActivity
import io.github.code_akram.or2.R
import io.github.code_akram.or2.app.Or2Application
import io.github.code_akram.or2.ffi.networkChanged
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel

/**
 * The foreground service that owns the application's connections while any host or session is
 * open (type `specialUse`: there is no category for an interactive SSH/mosh client). The process
 * holds the one `HostConnections` ([Or2Application.connections]); this service keeps the process
 * foreground, shows the ongoing notification (hosts, sessions, "Disconnect all"), forwards
 * default-network changes to `network_changed()` and stops itself when the last host or session
 * closes ([ServiceController]).
 */
class ConnectionService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private lateinit var controller: ServiceController
    private var network: NetworkWatch? = null

    private val host = object : ServiceHost {
        override fun show(content: NotificationContent) {
            val notification = buildNotification(content)
            startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        }

        override fun stop() {
            stopForeground(STOP_FOREGROUND_REMOVE)
            stopSelf()
        }
    }

    override fun onCreate() {
        super.onCreate()
        val app = application as Or2Application
        createChannel()
        controller = ServiceController(scope, app.connections.serviceSnapshots(), host)
        network = NetworkWatch(this, scope).also { it.start() }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val app = application as Or2Application
        // Start-up first: a foreground service must post its notification at once.
        controller.begin()
        if (intent?.action == ACTION_DISCONNECT_ALL) app.connections.disconnectAll()
        // The process is the owner: a killed process has no connections to restore.
        return START_NOT_STICKY
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onDestroy() {
        network?.stop()
        network = null
        controller.close()
        scope.cancel()
        super.onDestroy()
        // Stopped from outside while connections are open (it stops itself only when nothing is): start it again.
        (application as Or2Application).reviveService()
    }

    private fun createChannel() {
        val channel = NotificationChannel(CHANNEL_ID, "Connections", NotificationManager.IMPORTANCE_LOW).apply {
            description = "Shown while or2 holds SSH or mosh connections open."
            setShowBadge(false)
        }
        getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
    }

    private fun buildNotification(content: NotificationContent): Notification {
        val open = PendingIntent.getActivity(
            this, 0, Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val disconnect = PendingIntent.getService(
            this, 1, disconnectAllIntent(this), PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val style = Notification.InboxStyle().also { inbox -> content.lines.forEach(inbox::addLine) }
        return Notification.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_stat_or2)
            .setContentTitle(content.title)
            .setContentText(content.text)
            .setStyle(style)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setCategory(Notification.CATEGORY_SERVICE)
            .setContentIntent(open)
            .addAction(Notification.Action.Builder(null, "Disconnect all", disconnect).build())
            .build()
    }

    companion object {
        const val CHANNEL_ID = "connections"
        const val NOTIFICATION_ID = 1
        const val ACTION_DISCONNECT_ALL = "io.github.code_akram.or2.action.DISCONNECT_ALL"

        fun startIntent(context: Context) = Intent(context, ConnectionService::class.java)

        fun disconnectAllIntent(context: Context) = startIntent(context).setAction(ACTION_DISCONNECT_ALL)

        /** Starts the service; call while the app is in the foreground. Never throws. */
        fun start(context: Context) {
            try {
                context.startForegroundService(startIntent(context))
            } catch (_: IllegalStateException) {
                // Not allowed to start from the background: connections only begin in the foreground,
                // so this means the app just left it. The next connection starts the service.
            }
        }
    }
}

/**
 * Registers for default-network changes while the service runs and reports each one, debounced by
 * 500 ms ([NetworkChanges]), as `network_changed()`: live mosh sessions open a new socket and every
 * SSH connection sends a keepalive at once.
 */
class NetworkWatch(private val context: Context, private val scope: CoroutineScope) {
    private val manager = context.getSystemService(ConnectivityManager::class.java)
    private var changes: NetworkChanges? = null
    private var callback: ConnectivityManager.NetworkCallback? = null

    fun start() {
        val tracker = NetworkChanges(scope, manager.activeNetwork?.networkHandle) { networkChanged() }
        changes = tracker
        val registered = object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) = tracker.available(network.networkHandle)
            override fun onLost(network: Network) = tracker.lost(network.networkHandle)
        }
        callback = registered
        // On the main looper: the tracker and its debounce job are main-thread state.
        manager.registerDefaultNetworkCallback(registered, Handler(Looper.getMainLooper()))
    }

    fun stop() {
        callback?.let { runCatching { manager.unregisterNetworkCallback(it) } }
        callback = null
        changes = null
    }
}
