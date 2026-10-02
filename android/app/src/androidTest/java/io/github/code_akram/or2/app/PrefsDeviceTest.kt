package io.github.code_akram.or2.app

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.github.code_akram.or2.connection.MoshServerLedger
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.HostEndpoint
import io.github.code_akram.or2.data.HostRecord
import io.github.code_akram.or2.data.TransportPref
import java.io.File
import org.junit.After
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

/** `SharedPrefsStore.putStringDurably` and the mosh-server ledger on it: the file holds a record when the call returns. */
@RunWith(AndroidJUnit4::class)
class PrefsDeviceTest {
    private val context = InstrumentationRegistry.getInstrumentation().targetContext
    private val file = "or2-prefs-device-test"

    private fun onDisk(): String = File(context.dataDir, "shared_prefs/$file.xml").takeIf { it.exists() }?.readText().orEmpty()

    @After
    fun cleanUp() {
        context.deleteSharedPreferences(file)
    }

    @Test
    fun aDurableWriteIsInTheFileWhenItReturns() {
        val store = SharedPrefsStore(context, file)
        store.putStringDurably("durable", "written-now")
        assertTrue(onDisk().contains("written-now"))
    }

    @Test
    fun aMoshServerRecordIsInTheFileWhenRecordReturns() {
        val host = Host(HostRecord(7, "Fixture", "fixture", null, true, TransportPref.AUTO, false, 0), listOf(HostEndpoint("fixture.invalid", 22)))
        MoshServerLedger(SharedPrefsStore(context, file)).record(host, 4242u)
        assertTrue(onDisk().contains("7:4242:"))
    }
}
