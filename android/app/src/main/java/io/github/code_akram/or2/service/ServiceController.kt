package io.github.code_akram.or2.service

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.launch
import java.util.concurrent.atomic.AtomicInteger

/**
 * What the Android service delegates to: showing the ongoing notification and ending itself.
 * Kept apart from `Service` so the ownership rules below run on the JVM.
 */
interface ServiceHost {
    /** Shows or updates the ongoing notification (the first call also makes the service foreground). */
    fun show(content: NotificationContent)

    /** Removes the notification and ends the service. */
    fun stop()
}

/**
 * The foreground service's lifetime rules. The service owns the application's connections while
 * any host or session is open: it keeps its notification in step with them, and stops itself when
 * the last one closes. [begin] must be called from `onStartCommand`; the first notification is
 * shown at once (Android demands a foreground service post one within seconds of starting), even
 * if nothing turns out to be open, and the service then stops on the first idle snapshot.
 */
class ServiceController(
    private val scope: CoroutineScope,
    private val snapshots: Flow<ServiceSnapshot>,
    private val host: ServiceHost,
    private val state: ServiceRunState = ServiceRunState.Process,
) {
    private var job: Job? = null

    /** The snapshot the notification currently reflects. */
    var current: ServiceSnapshot = ServiceSnapshot(emptyList())
        private set

    fun begin() {
        state.running = true
        state.begins.incrementAndGet()
        host.show(notificationContent(current))
        if (job != null) return
        job = scope.launch {
            snapshots.collect { snapshot ->
                current = snapshot
                if (snapshot.idle) end() else host.show(notificationContent(snapshot))
            }
        }
    }

    /** Called when the service is destroyed for any reason. */
    fun close() {
        state.running = false
        job?.cancel()
        job = null
    }

    private fun end() {
        // No longer running before the stop is requested, so a connection opened right now starts a new one.
        state.running = false
        job?.cancel()
        job = null
        host.stop()
    }
}

/** Whether the service is running, shared by the service and the code that starts it. */
class ServiceRunState {
    @Volatile
    var running = false

    /** How many times the service has begun (`onStartCommand`): lets a test tell "never started" from "started, then stopped". */
    val begins = AtomicInteger()

    companion object {
        /** The process's one service. */
        val Process = ServiceRunState()
    }
}

/**
 * Starts the service whenever something is open and it is not running (a connection began, or the
 * service stopped a moment ago and the user connected again). Feed it [ServiceSnapshot]s; it calls
 * [start] on the main thread. A snapshot only arrives when the set of open things changes, so
 * [recheck] covers the other way to end up with connections and no service: the service was
 * destroyed from outside (the user stopped it, or a start was swallowed because the app was in the
 * background). The app calls it when the service is destroyed and when it returns to the foreground.
 */
class ServiceStarter(private val state: ServiceRunState = ServiceRunState.Process, private val start: () -> Unit) {
    private var latest: ServiceSnapshot? = null

    fun onSnapshot(snapshot: ServiceSnapshot) {
        latest = snapshot
        recheck()
    }

    /** Starts the service if the latest snapshot has something open and it is not running. */
    fun recheck() {
        val snapshot = latest ?: return
        if (!snapshot.idle && !state.running) start()
    }
}

/**
 * Turns the connectivity callback's events into one debounced `network_changed()`. Four things
 * count as a change, and several within [debounceMs] collapse into one notification:
 *
 * - the default network is a different one ([available]; the callback's first report of the network
 *   that was already the default is not a change), or the same one came back after [lost];
 * - its **transport set** changed ([capabilitiesChanged]: Wi-Fi to cellular under a VPN keeps the
 *   same default network, and this is the only signal), or its **interface** changed
 *   ([linkChanged]). The first report of either is only the baseline, and a report of the same
 *   value is not a change, so the stream of bandwidth and signal-strength updates that arrives
 *   through the same callbacks does nothing;
 * - the app returned to the foreground ([foregrounded]): whatever happened while it was away is
 *   settled by one roam, whether or not the callbacks fired in the background.
 */
class NetworkChanges(
    private val scope: CoroutineScope,
    initial: Long?,
    private val debounceMs: Long = DEBOUNCE_MS,
    private val notify: () -> Unit,
) {
    private var current = initial
    private var transports: String? = null
    private var iface: String? = null
    private var pending: Job? = null

    /** Starts tracking [network] as the default without counting it as a change. */
    fun seed(network: Long?) {
        current = network
        transports = null
        iface = null
    }

    fun available(network: Long) {
        if (network == current) return
        current = network
        transports = null
        iface = null
        schedule()
    }

    fun lost(network: Long) {
        if (network == current) {
            current = null
            transports = null
            iface = null
        }
    }

    /** [signature] names the network's transports (for example `CELLULAR,VPN`). */
    fun capabilitiesChanged(network: Long, signature: String) {
        if (network != current) available(network)
        val before = transports
        transports = signature
        if (before != null && before != signature) schedule()
    }

    /** [name] is the link's interface name (`wlan0`, `rmnet_data1`, `tun0`), null when it has none. */
    fun linkChanged(network: Long, name: String?) {
        if (network != current) available(network)
        val before = iface
        iface = name ?: ""
        if (before != null && before != iface) schedule()
    }

    /** The app is back in front of the user. */
    fun foregrounded() = schedule()

    private fun schedule() {
        pending?.cancel()
        pending = scope.launch {
            delay(debounceMs)
            notify()
        }
    }

    companion object {
        const val DEBOUNCE_MS = 500L
    }
}
