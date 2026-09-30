package io.github.code_akram.or2.keys

import android.os.Bundle
import androidx.biometric.BiometricManager
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.security.KeyStore
import java.util.UUID

/** Does not bypass biometrics or change enrollment. Successful CryptoObject use is a manual check. */
@RunWith(AndroidJUnit4::class)
class VaultDeviceTest {
    @Test
    fun hardwarePerUsePolicyRejectsEncryptionWithoutAuthentication() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        assumeTrue("Strong biometric enrollment required", BiometricManager.from(context)
            .canAuthenticate(BiometricManager.Authenticators.BIOMETRIC_STRONG) == BiometricManager.BIOMETRIC_SUCCESS)
        val vault = BiometricVault(context)
        val id = UUID.randomUUID().toString()
        val bytes = byteArrayOf(3, 7, 2)
        try {
            val cipher = vault.createCipher(id)
            val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
            val key = store.getKey("or2.private.$id", null) as javax.crypto.SecretKey
            val duration = vault.verifyHardware(key).userAuthenticationValidityDurationSeconds
            InstrumentationRegistry.getInstrumentation().sendStatus(2, Bundle().apply {
                putString("stream", "Keystore per-use authentication validity: $duration seconds\n")
            })
            assertTrue("Expected per-use timeout, got $duration", duration == 0 || duration == -1)
            assertNull(key.encoded)
            assertThrows(Exception::class.java) { cipher.doFinal(bytes) }
        } finally { bytes.fill(0); vault.delete(id) }
        assertThrows(VaultException::class.java) { vault.decryptCipher(id, ByteArray(12)) }
    }
}
