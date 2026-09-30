package io.github.code_akram.or2.keys

import android.content.Context
import android.content.pm.PackageManager
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyInfo
import android.security.keystore.KeyPermanentlyInvalidatedException
import android.security.keystore.KeyProperties
import android.security.keystore.StrongBoxUnavailableException
import androidx.biometric.BiometricManager
import androidx.biometric.BiometricPrompt
import androidx.core.content.ContextCompat
import androidx.fragment.app.FragmentActivity
import kotlinx.coroutines.suspendCancellableCoroutine
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.SecretKeyFactory
import javax.crypto.spec.GCMParameterSpec
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException

class VaultException(override val message: String) : Exception(message)

/** One non-exportable hardware AES key per key record; enrollment invalidation never rotates it. */
class BiometricVault(private val context: Context) {
    private val store get() = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
    private fun alias(id: String) = "or2.private.$id"

    fun createCipher(id: String): Cipher {
        requireEnrollment()
        val strongBox = context.packageManager.hasSystemFeature(PackageManager.FEATURE_STRONGBOX_KEYSTORE)
        val key = try {
            generate(id, strongBox)
        } catch (_: StrongBoxUnavailableException) {
            generate(id, false)
        }
        try {
            verifyHardware(key)
            return Cipher.getInstance("AES/GCM/NoPadding").apply { init(Cipher.ENCRYPT_MODE, key) }
        } catch (error: Exception) {
            delete(id)
            throw error
        }
    }

    fun decryptCipher(id: String, iv: ByteArray): Cipher {
        requireEnrollment()
        val key = store.getKey(alias(id), null) as? SecretKey
            ?: throw VaultException("The encryption key is missing. Re-import or generate a new SSH key; this record cannot be recovered.")
        verifyHardware(key)
        return Cipher.getInstance("AES/GCM/NoPadding").apply {
            init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, iv))
        }
    }

    fun delete(id: String) { store.deleteEntry(alias(id)) }

    internal fun verifyHardware(key: SecretKey): KeyInfo {
        val info = SecretKeyFactory.getInstance(key.algorithm, "AndroidKeyStore")
            .getKeySpec(key, KeyInfo::class.java) as KeyInfo
        if (info.securityLevel != KeyProperties.SECURITY_LEVEL_STRONGBOX &&
            info.securityLevel != KeyProperties.SECURITY_LEVEL_TRUSTED_ENVIRONMENT
        ) throw VaultException("Hardware-backed encryption is unavailable. Software key storage is not allowed.")
        // Builder timeout 0 is per-use; KeyInfo represents per-use as -1, not 0.
        check(info.isUserAuthenticationRequired && info.userAuthenticationValidityDurationSeconds == -1 &&
            info.isUserAuthenticationRequirementEnforcedBySecureHardware &&
            info.userAuthenticationType == KeyProperties.AUTH_BIOMETRIC_STRONG &&
            info.isInvalidatedByBiometricEnrollment
        ) { "Keystore did not enforce the required biometric policy." }
        return info
    }

    private fun generate(id: String, strongBox: Boolean): SecretKey =
        KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").run {
            init(KeyGenParameterSpec.Builder(alias(id), KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setKeySize(256)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setUserAuthenticationRequired(true)
                .setUserAuthenticationParameters(0, KeyProperties.AUTH_BIOMETRIC_STRONG)
                .setInvalidatedByBiometricEnrollment(true)
                .setIsStrongBoxBacked(strongBox)
                .build())
            generateKey()
        }

    fun requireEnrollment() {
        when (BiometricManager.from(context).canAuthenticate(BiometricManager.Authenticators.BIOMETRIC_STRONG)) {
            BiometricManager.BIOMETRIC_SUCCESS -> Unit
            BiometricManager.BIOMETRIC_ERROR_NONE_ENROLLED -> throw VaultException("Enroll a strong biometric in Android settings before saving or unlocking keys.")
            BiometricManager.BIOMETRIC_ERROR_HW_UNAVAILABLE -> throw VaultException("Biometric hardware is temporarily unavailable. Try again later.")
            else -> throw VaultException("Strong biometric authentication is unavailable on this device. Device credentials are not a fallback.")
        }
    }
}

/** Invoke on main. Cancellation (including Activity destruction) cancels the prompt. */
suspend fun authenticateCipher(activity: FragmentActivity, cipher: Cipher, title: String): Cipher =
    suspendCancellableCoroutine { continuation ->
        val prompt = BiometricPrompt(activity, ContextCompat.getMainExecutor(activity),
            object : BiometricPrompt.AuthenticationCallback() {
                override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                    if (continuation.isActive) {
                        val authenticated = result.cryptoObject?.cipher
                        if (authenticated == null) continuation.resumeWithException(VaultException("Biometric authentication did not unlock the cipher."))
                        else continuation.resume(authenticated)
                    }
                }

                override fun onAuthenticationError(code: Int, message: CharSequence) {
                    if (continuation.isActive) continuation.resumeWithException(VaultException(when (code) {
                        BiometricPrompt.ERROR_USER_CANCELED, BiometricPrompt.ERROR_CANCELED,
                        BiometricPrompt.ERROR_NEGATIVE_BUTTON -> "Biometric authentication cancelled. No key was saved or connection opened."
                        BiometricPrompt.ERROR_NO_BIOMETRICS -> "Enroll a strong biometric in Android settings first."
                        BiometricPrompt.ERROR_LOCKOUT, BiometricPrompt.ERROR_LOCKOUT_PERMANENT -> "Biometric authentication is locked. Unlock the device and try again later."
                        else -> "Biometric authentication unavailable. Try again later."
                    }))
                }
            })
        continuation.invokeOnCancellation { activity.runOnUiThread { prompt.cancelAuthentication() } }
        prompt.authenticate(BiometricPrompt.PromptInfo.Builder()
            .setTitle(title)
            .setSubtitle("Strong biometric required for this key operation")
            .setAllowedAuthenticators(BiometricManager.Authenticators.BIOMETRIC_STRONG)
            .setNegativeButtonText("Cancel")
            .build(), BiometricPrompt.CryptoObject(cipher))
    }

fun vaultErrorMessage(error: Exception): String = when (error) {
    is KeyPermanentlyInvalidatedException -> "Biometric enrollment changed and invalidated this key. Re-import or generate a new key, then update the host's key selection. The old record can be deleted."
    is VaultException -> error.message
    else -> "The encrypted key could not be used. Retry, or re-import/generate a new key if it is no longer recoverable."
}
