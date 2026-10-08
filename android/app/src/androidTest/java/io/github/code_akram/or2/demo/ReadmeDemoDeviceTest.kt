package io.github.code_akram.or2.demo

import android.os.SystemClock
import android.util.Log
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.text.AnnotatedString
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.github.code_akram.or2.MainActivity
import io.github.code_akram.or2.app.Or2Application
import io.github.code_akram.or2.app.SharedPrefsStore
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.HostEndpoint
import io.github.code_akram.or2.data.HostRecord
import io.github.code_akram.or2.data.KeyRecord
import io.github.code_akram.or2.ffi.HerdrState
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.PublicKeyInfo
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.service.ConnectionService
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import org.json.JSONObject
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

/**
 * Records the README's demo: the real app, driven by a finger through a made-up world ([DemoWorld]), filmed as it
 * runs. Not a test of anything: it runs only when asked (`-e or2.demo 1`, see docs/build.md, "README demo") and is
 * skipped in every suite run.
 *
 * It writes the app's `files/demo/`: `frames/` (JPEGs `or2.demo.width` pixels wide, default 1080, and `frames.txt`
 * giving each one's time) and `timeline.json` (the screen, the scenes and every touch, on the frames' clock), which
 * `cargo xtask demo-gif` turns into `docs/media/or2-demo.gif`. The device-test app must have no stored hosts; the demo's
 * hosts, key and settings are removed and restored afterwards.
 */
@RunWith(AndroidJUnit4::class)
class ReadmeDemoDeviceTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val args = InstrumentationRegistry.getArguments()
    private val context = instrumentation.targetContext
    private val app = context.applicationContext as Or2Application

    @Test
    fun recordTheReadmeDemo() {
        assumeTrue("The README demo records only with -e or2.demo 1", args.getString("or2.demo") == "1")
        val dao = app.database.dao()
        check(runBlocking { dao.hosts().first() }.isEmpty()) { "The device-test app has stored hosts; the demo needs it empty" }

        // The app's own settings, put back afterwards: no offers on Home, no agent notifications, nothing to resume.
        val prefs = context.getSharedPreferences(SharedPrefsStore.DEFAULT_FILE, 0)
        val saved = prefs.all.toMap()
        prefs.edit().clear().commit()
        app.notifications.offer.dismiss()
        app.agentAlertSettings.set(false)

        val world = DemoWorld()
        val driver = DemoDriver()
        val ids = runBlocking {
            dao.insertKey(DEMO_KEY)
            listOf(world.workstation, world.buildBox).map { host ->
                dao.saveHostWithTrust(
                    Host(HostRecord(label = host.label, username = "dev", keyId = DEMO_KEY.id), listOf(HostEndpoint(host.address, 22))),
                    DEMO_HOST_KEY,
                )
            }
        }
        val (atlasId, buildBoxId) = ids
        var scenario: ActivityScenario<MainActivity>? = null
        var recorder: DemoDriver.Recorder? = null
        try {
            app.connectorOverride = world.connector
            app.watchConnections()
            // Connected before the app opens (no key unlock on camera), with a few terminals already open: what Home
            // looks like on a working day.
            runBlocking(Dispatchers.Main) { app.connections.connect(ids.map { dao.host(it)!! }, byteArrayOf(1)) }
            awaitTrue("both hosts connected") { ids.all { app.connections.host(it)?.state?.value is HostState.Connected } }
            awaitTrue("herdr views") {
                ids.all { id -> app.connections.host(id)?.watches?.value?.any { it.state.value is HerdrState.Live } == true }
            }
            world.startAmbient()
            instrumentation.runOnMainSync {
                val atlas = app.connections.host(atlasId)!!
                val buildBox = app.connections.host(buildBoxId)!!
                app.connections.openTerminal(atlas, TerminalTarget.Herdr(null, null))
                app.connections.openTerminal(atlas, TerminalTarget.Tmux("main"))
                app.connections.openTerminal(buildBox, TerminalTarget.Herdr(null, null))
            }
            SystemClock.sleep(1_500) // Mosh takes over the opened terminals.

            val activity = ActivityScenario.launch(MainActivity::class.java).also { scenario = it }
            driver.await("Home", match = driver.tag("host-card:$atlasId"))
            SystemClock.sleep(1_200)
            val out = File(context.filesDir, "demo").apply { deleteRecursively(); mkdirs() }
            val frameWidth = args.getString("or2.demo.width")?.toInt() ?: 1080
            val screen = screenOf(activity)
            recorder = driver.Recorder(File(out, "frames"), frameWidth)
            driver.t0 = SystemClock.uptimeMillis()
            recorder.start()
            SystemClock.sleep(400)

            tour(world, driver, atlasId, buildBoxId)

            SystemClock.sleep(600)
            Log.i(TAG, "Recorded ${recorder.stop()}")
            recorder = null
            File(out, "timeline.json").writeText(driver.timeline(screen, frameWidth).toString(2))
        } finally {
            recorder?.stop()
            world.stop()
            scenario?.close()
            runBlocking(Dispatchers.Main) { ids.forEach { app.connections.release(it, closeTerminals = true) } }
            context.stopService(ConnectionService.startIntent(context))
            app.connectorOverride = null
            runBlocking {
                ids.forEach { dao.deleteHost(it) }
                dao.deleteKey(DEMO_KEY.id)
            }
            prefs.edit().clear().apply {
                saved.forEach { (name, value) ->
                    when (value) {
                        is Boolean -> putBoolean(name, value)
                        is Int -> putInt(name, value)
                        is Long -> putLong(name, value)
                        is Float -> putFloat(name, value)
                        is String -> putString(name, value)
                        is Set<*> -> putStringSet(name, value.filterIsInstance<String>().toSet())
                    }
                }
            }.commit()
        }
    }

    /**
     * The storyboard: six scenes, each a caption of the GIF. Pauses are for a viewer: each scene holds long enough to be
     * read once, and it starts and ends on Home so the GIF loops without a jump.
     */
    private fun tour(world: DemoWorld, driver: DemoDriver, atlasId: Long, buildBoxId: Long) {
        driver.scene(1, "Your hosts, with live terminals")
        SystemClock.sleep(2_900)

        driver.scene(2, "Every agent in one inbox")
        driver.tap("the inbox", driver.tag("nav-inbox"))
        SystemClock.sleep(1_500)
        world.codexFinishes()
        SystemClock.sleep(2_100)

        driver.scene(3, "Tap a blocked agent to jump in")
        driver.tap("the blocked agent", driver.tag("inbox-item:$atlasId:-:${world.claude.paneId}"))
        SystemClock.sleep(2_400)

        driver.scene(4, "Reply from the composer")
        driver.tap("the composer toggle", driver.tag("key:Composer"))
        SystemClock.sleep(900)
        type(driver, "composer-input", "1, then run the tests")
        SystemClock.sleep(450)
        driver.tap("send", driver.tag("composer-send"))
        SystemClock.sleep(700)
        driver.tap("close the composer", driver.tag("composer-close"))
        SystemClock.sleep(350)
        driver.tap("hide the keyboard", driver.tag("key:Keyboard"))
        SystemClock.sleep(3_700)

        driver.scene(5, "Switch herdr spaces and panes")
        driver.tap("Spaces", driver.tag("terminal-spaces"))
        SystemClock.sleep(1_700)
        driver.tap("Codex", driver.tag("space-agent:${world.codex.paneId}"))
        SystemClock.sleep(2_300)

        driver.scene(6, "Open tmux, herdr or a shell")
        driver.tap("Home", driver.tag("terminal-back"))
        SystemClock.sleep(1_300)
        driver.tap("build-box", driver.tag("host:$buildBoxId"))
        SystemClock.sleep(1_500)
        driver.tap("the tmux tab", driver.text("tmux"))
        SystemClock.sleep(1_200)
        driver.tap("the dirs tab", driver.text("dirs"))
        SystemClock.sleep(1_400)
        val (x, y) = driver.await("the picker", match = driver.tag("picker-title"))
        driver.drag(x, y, 1_400f, 260)
        SystemClock.sleep(1_600)
    }

    /** Types [text] into the field tagged [tag] a character at a time, at a quick typist's pace. */
    private fun type(driver: DemoDriver, tag: String, text: String) {
        for (end in 1..text.length) {
            instrumentation.runOnMainSync {
                driver.node(driver.tag(tag))?.config?.getOrNull(SemanticsActions.SetText)?.action?.invoke(AnnotatedString(text.take(end)))
            }
            SystemClock.sleep(if (text[end - 1] == ' ') 110 else 62)
        }
    }

    /** The display in pixels, and the status and navigation bars the GIF crops away. */
    private fun screenOf(scenario: ActivityScenario<MainActivity>): JSONObject {
        var screen: JSONObject? = null
        scenario.onActivity { activity ->
            val decor = activity.window.decorView
            val insets = ViewCompat.getRootWindowInsets(decor)!!
            val top = insets.getInsets(WindowInsetsCompat.Type.statusBars() or WindowInsetsCompat.Type.displayCutout()).top
            screen = JSONObject().put("width", decor.width).put("height", decor.height).put("statusBar", top)
                .put("navigationBar", insets.getInsets(WindowInsetsCompat.Type.navigationBars()).bottom)
        }
        return screen!!
    }

    private fun awaitTrue(what: String, timeoutMs: Long = 10_000, condition: () -> Boolean) {
        val deadline = SystemClock.uptimeMillis() + timeoutMs
        while (true) {
            var ok = false
            instrumentation.runOnMainSync { ok = condition() }
            if (ok) return
            check(SystemClock.uptimeMillis() < deadline) { "Timed out waiting for $what" }
            SystemClock.sleep(50)
        }
    }

    private companion object {
        const val TAG = "or2.demo"

        /** A key record that is never unlocked: the demo's hosts are connected before the app opens. */
        val DEMO_KEY = KeyRecord(
            "or2-demo-key", "Phone key", "ssh-ed25519", "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIDemoKeyDemoKeyDemoKeyDemoKeyDemoKey00 phone",
            "SHA256:Dm0Kq2VxR8nL3aTp0sWZ4cEdHfY8uJbNqXoGiVtB1Mk", "phone", byteArrayOf(), byteArrayOf(),
        )
        val DEMO_HOST_KEY = PublicKeyInfo(
            "ssh-ed25519", "AAAAC3NzaC1lZDI1NTE5AAAAIDemoHostKeyDemoHostKeyDemoHostKey0", "SHA256:Hk7Lq0VxR8nL3aTp0sWZ4cEdHfY8uJbNqXoGiVtB1Mk", "",
        )
    }
}
