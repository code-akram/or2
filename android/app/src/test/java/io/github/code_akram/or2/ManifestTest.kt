package io.github.code_akram.or2

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

/**
 * The manifest is the contract with the system for staying connected: the foreground service and its
 * type, the notification, the network callback and the one-time battery exemption request. A missing
 * permission fails at runtime on a phone (a `SecurityException` from `startForeground`, no callback),
 * so the list is pinned here. The app must also stay F-Droid clean: no Play Services, no FCM.
 */
class ManifestTest {
    private val manifest = File("src/main/AndroidManifest.xml").readText()

    private val permissions get() = Regex("<uses-permission android:name=\"([^\"]+)\"").findAll(manifest).map { it.groupValues[1] }.toList()

    @Test
    fun everyPermissionTheServiceAndTheRoamingNeedIsDeclared() {
        val declared = permissions.toSet()
        for (name in listOf(
            "FOREGROUND_SERVICE", "FOREGROUND_SERVICE_SPECIAL_USE", "POST_NOTIFICATIONS", "ACCESS_NETWORK_STATE",
            "REQUEST_IGNORE_BATTERY_OPTIMIZATIONS", "INTERNET", "USE_BIOMETRIC", "CAMERA",
        )) {
            assertTrue("android.permission.$name is missing", "android.permission.$name" in declared)
        }
    }

    @Test
    fun noPermissionIsDeclaredTwiceAndNothingBeyondTheKnownListIsAsked() {
        assertEquals(permissions.toSet().size, permissions.size)
        assertEquals(8, permissions.size) // A new permission is a decision: update this list and the docs.
    }

    @Test
    fun theCameraIsOptionalHardwareSoPastingStillWorksWithoutOne() {
        // CAMERA is for Easy pair's scanner only; a device without a camera (or one that denies it) can paste the code.
        for (feature in listOf("android.hardware.camera", "android.hardware.camera.autofocus")) {
            val tag = Regex("<uses-feature android:name=\"${Regex.escape(feature)}\"[^>]*/>").find(manifest)?.value
            assertTrue("$feature must be declared", tag != null)
            assertTrue("$feature must not be required", tag!!.contains("android:required=\"false\""))
        }
    }

    @Test
    fun theServiceIsAForegroundServiceOfTheSpecialUseTypeWithItsSubtype() {
        val service = Regex("<service[^>]*ConnectionService[\\s\\S]*?</service>").find(manifest)!!.value
        assertTrue(service.contains("android:foregroundServiceType=\"specialUse\""))
        assertTrue(service.contains("android.app.PROPERTY_SPECIAL_USE_FGS_SUBTYPE"))
        assertTrue(service.contains("android:exported=\"false\""))
    }

    @Test
    fun noGooglePlayServicesOrFcm() {
        assertFalse(manifest.contains("com.google.android.gms"))
        assertFalse(manifest.contains("com.google.firebase"))
        assertFalse(manifest.contains("c2dm"))
    }
}
