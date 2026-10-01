package io.github.code_akram.or2.service

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.launch

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

    companion object {
        /** The process's one service. */
        val Process = ServiceRunState()
    }
}

/**
 * Starts the service whenever something is open and it is not running (a connection began, or the
 * service stopped a moment ago and the user connected again). Feed it [ServiceSnapshot]s; it calls
 * [start] on the main thread.
 */
class ServiceStarter(private val state: ServiceRunState = ServiceRunState.Process, private val start: () -> Unit) {
    fun onSnapshot(snapshot: ServiceSnapshot) {
        if (!snapshot.idle && !state.running) start()
    }
}

/**
 * Turns the connectivity callback's events into one debounced `network_changed()`. The default
 * network is tracked by its handle so only a real change counts: [available] for the network that
 * is already the default (a repeat, or the callback's first report after registering) does nothing,
 * a different one does, and [lost] followed by the same network returning does (lost-then-available).
 * Several events within [debounceMs] collapse into one notification.
 */
class NetworkChanges(
    private val scope: CoroutineScope,
    initial: Long?,
    private val debounceMs: Long = DEBOUNCE_MS,
    private val notify: () -> Unit,
) {
    private var current = initial
    private var pending: Job? = null

    fun available(network: Long) {
        if (network == current) return
        current = network
        schedule()
    }

    fun lost(network: Long) {
        if (network == current) current = null
    }

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
