package io.github.code_akram.or2.service

import android.Manifest
import android.app.NotificationManager
import android.content.Context
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.github.code_akram.or2.app.Or2Application
import io.github.code_akram.or2.connection.HostConnector
import io.github.code_akram.or2.connection.UiPort
import io.github.code_akram.or2.connection.uiHost
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalTarget
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

/**
 * The foreground service on a device, over a scripted connection (no network, no Keystore, no
 * stored host): it starts when a host connects, posts the ongoing notification with "Disconnect
 * all", follows the open sessions, and stops itself once everything is closed.
 */
@RunWith(AndroidJUnit4::class)
class ConnectionServiceDeviceTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context = instrumentation.targetContext
    private val app = context.applicationContext as Or2Application
    private val host = uiHost(id = 9001, label = "Service fixture")
    private val ports = mutableListOf<UiPort>()

    @Before
    fun setUp() {
        // The notification is only visible with the permission; the service itself never needs it.
        instrumentation.uiAutomation.grantRuntimePermission(context.packageName, Manifest.permission.POST_NOTIFICATIONS)
        app.connectorOverride = HostConnector { _, listener ->
            UiPort().also {
                it.hostListener = listener
                it.native = HostState.Connected(0u)
                ports += it
                listener.onHostStateChanged(HostState.Connected(0u))
            }
        }
        app.watchConnections()
    }

    @After
    fun tearDown() {
        runBlocking(Dispatchers.Main) { app.connections.release(host.id, closeTerminals = true) }
        context.stopService(ConnectionService.startIntent(context))
        app.connectorOverride = null
    }

    private fun await(what: String, timeoutMs: Long = 10_000, condition: () -> Boolean) {
        val deadline = System.currentTimeMillis() + timeoutMs
        while (!condition()) {
            if (System.currentTimeMillis() > deadline) throw AssertionError("Timed out waiting for $what")
            Thread.sleep(50)
        }
    }

    private fun notification() = context.getSystemService(NotificationManager::class.java).activeNotifications
        .firstOrNull { it.id == ConnectionService.NOTIFICATION_ID }?.notification

    @Test
    fun theServiceStartsWithAHostShowsItsSessionsAndStopsWhenEverythingIsClosed() {
        runBlocking(Dispatchers.Main) { app.connections.connect(host, byteArrayOf(1)) }
        await("the service to run") { ServiceRunState.Process.running }
        await("the notification") { notification() != null }
        val shown = notification()!!
        assertEquals("Connected to Service fixture", shown.extras.getString("android.title"))
        assertEquals(listOf("Disconnect all"), shown.actions.map { it.title.toString() })
        assertTrue(shown.flags and android.app.Notification.FLAG_ONGOING_EVENT != 0)

        // A session joins the count.
        runBlocking(Dispatchers.Main) {
            val active = app.connections.host(host.id)!!
            app.connections.openTerminal(active, TerminalTarget.Shell)
        }
        await("the notification to count the session") { notification()?.extras?.getString("android.text") == "1 open session" }

        // "Disconnect all" (the notification action's own intent): everything closes, the service stops.
        context.startService(ConnectionService.disconnectAllIntent(context))
        await("the service to stop") { !ServiceRunState.Process.running }
        await("the notification to go") { notification() == null }
        runBlocking(Dispatchers.Main) {
            assertTrue(app.connections.host(host.id)!!.state.value is HostState.Closed)
            assertTrue(app.connections.terminals.value.all { it.state.value is SessionState.Closed })
            assertFalse(app.connections.hasOpenSession())
        }
        assertNotNull(ports.firstOrNull())
    }

    @Test
    fun aServiceStartedWithNothingOpenStopsItself() {
        // It must still post its notification at once (Android's deadline), then notice there is nothing to hold.
        context.startForegroundService(ConnectionService.startIntent(context))
        Thread.sleep(2_000)
        assertFalse(ServiceRunState.Process.running)
        assertTrue(notification() == null)
    }
}
