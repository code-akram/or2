package io.github.code_akram.or2.service

import android.Manifest
import android.app.NotificationManager
import android.content.pm.PackageManager
import android.os.Build
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
import org.junit.Assume.assumeTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

/**
 * The foreground service on a device, over a scripted connection (no network, no Keystore, no
 * stored host): it starts when a host connects, owns the connections while any is open ("Disconnect
 * all" closes them all), and stops itself once everything is closed. Those are asserted
 * unconditionally.
 *
 * The notification's contents are asserted only where the app already holds the notification
 * permission. The test never grants it: some OEM builds (OxygenOS) refuse `pm grant` to the shell
 * (`GRANT_RUNTIME_PERMISSIONS`), and the service runs without it anyway. Without the permission the
 * notification test is skipped with a message (not failed); grant it by hand once to run it.
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

    /** Whether this install may post notifications; never changed by the test. */
    private fun notificationsVisible(): Boolean {
        val granted = Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU ||
            context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED
        return granted && context.getSystemService(NotificationManager::class.java).areNotificationsEnabled()
    }

    private fun assumeNotificationsVisible() = assumeTrue(
        "POST_NOTIFICATIONS is not granted to ${context.packageName} (and the test never grants it: OEM builds " +
            "refuse shell grants); skipping the notification contents. Grant it in Settings to run this test.",
        notificationsVisible(),
    )

    private fun notification() = context.getSystemService(NotificationManager::class.java).activeNotifications
        .firstOrNull { it.id == ConnectionService.NOTIFICATION_ID }?.notification

    @Test
    fun theServiceStartsWithAHostOwnsItsSessionsAndStopsWhenEverythingIsClosed() {
        runBlocking(Dispatchers.Main) { app.connections.connect(host, byteArrayOf(1)) }
        await("the service to run") { ServiceRunState.Process.running }

        // A session joins; the service keeps owning the connections while any is open.
        runBlocking(Dispatchers.Main) {
            val active = app.connections.host(host.id)!!
            app.connections.openTerminal(active, TerminalTarget.Shell)
            assertTrue(app.connections.hasOpenSession())
        }
        assertTrue("the service runs while a session is open", ServiceRunState.Process.running)

        // "Disconnect all" (the notification action's own intent): everything closes, the service stops.
        context.startService(ConnectionService.disconnectAllIntent(context))
        await("the service to stop") { !ServiceRunState.Process.running }
        runBlocking(Dispatchers.Main) {
            assertTrue(app.connections.host(host.id)!!.state.value is HostState.Closed)
            assertTrue(app.connections.terminals.value.all { it.state.value is SessionState.Closed })
            assertFalse(app.connections.hasOpenSession())
        }
        assertNotNull(ports.firstOrNull())
    }

    @Test
    fun theOngoingNotificationShowsTheHostAndItsSessionsWithDisconnectAll() {
        assumeNotificationsVisible()
        runBlocking(Dispatchers.Main) { app.connections.connect(host, byteArrayOf(1)) }
        await("the service to run") { ServiceRunState.Process.running }
        await("the notification") { notification() != null }
        val shown = notification()!!
        assertEquals("Connected to Service fixture", shown.extras.getString("android.title"))
        assertEquals(listOf("Disconnect all"), shown.actions.map { it.title.toString() })
        assertTrue(shown.flags and android.app.Notification.FLAG_ONGOING_EVENT != 0)

        runBlocking(Dispatchers.Main) {
            val active = app.connections.host(host.id)!!
            app.connections.openTerminal(active, TerminalTarget.Shell)
        }
        await("the notification to count the session") { notification()?.extras?.getString("android.text") == "1 open session" }

        context.startService(ConnectionService.disconnectAllIntent(context))
        await("the service to stop") { !ServiceRunState.Process.running }
        await("the notification to go") { notification() == null }
    }

    @Test
    fun aServiceStartedWithNothingOpenStopsItself() {
        // It must still post its notification at once (Android's deadline), then notice there is nothing to hold.
        // First prove it really started (a refused start would also leave nothing running and no notification),
        // then that it stopped itself.
        val begun = ServiceRunState.Process.begins.get()
        context.startForegroundService(ConnectionService.startIntent(context))
        await("the service to start") { ServiceRunState.Process.begins.get() > begun }
        await("the service to stop itself") { !ServiceRunState.Process.running }
        // Without the permission no notification is ever posted, so there is nothing to wait for.
        if (notificationsVisible()) await("the notification to go") { notification() == null }
    }
}
