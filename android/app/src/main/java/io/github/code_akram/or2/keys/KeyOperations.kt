package io.github.code_akram.or2.keys

import io.github.code_akram.or2.data.KeyRecord
import io.github.code_akram.or2.ffi.ClientKeyMaterial
import io.github.code_akram.or2.ffi.KeyException
import io.github.code_akram.or2.ffi.importPrivateKey
import java.io.InputStream
import java.security.GeneralSecurityException
import javax.crypto.Cipher

/**
 * A fingerprint for a list row: `SHA256:7vK2mQ9x…tB1MkA`, ellipsized in the middle so it stays on
 * one line and still ends the way the full value does. The full value is in the key's own sheet.
 */
fun shortFingerprint(fingerprint: String, head: Int = 8, tail: Int = 6): String {
    val start = fingerprint.indexOf(':') + 1
    return if (fingerprint.length <= start + head + 1 + tail) fingerprint
    else fingerprint.take(start + head) + "…" + fingerprint.takeLast(tail)
}

// Owns/closes the stream. Copy only after close succeeds, so a provider close error cannot
// strand a newly allocated plaintext return value. Wipe staging on all exit paths.
fun readPrivateKey(input: InputStream): ByteArray {
    val buffer = ByteArray(1024 * 1024)
    try {
        val size = input.use {
            var size = 0
            while (size < buffer.size) {
                val count = it.read(buffer, size, buffer.size - size)
                if (count < 0) break
                size += count
            }
            if (size == buffer.size) require(it.read() == -1) { "Private key file exceeds 1 MiB." }
            size
        }
        return buffer.copyOf(size)
    } finally {
        buffer.fill(0)
    }
}

/** Owns returned Rust material until encryption finishes, fails or is cancelled. */
suspend fun encryptKey(id: String, label: String, material: ClientKeyMaterial, cipher: suspend () -> Cipher): KeyRecord = try {
    val unlocked = cipher()
    val ciphertext = unlocked.doFinal(material.privateKey)
    val info = material.publicKey
    KeyRecord(id, label, info.algorithm, info.openssh, info.fingerprint, info.comment, ciphertext, unlocked.iv)
} finally {
    material.privateKey.fill(0)
}

fun importAndWipe(bytes: ByteArray, passphrase: String?): ClientKeyMaterial = try {
    importPrivateKey(bytes, passphrase)
} finally {
    bytes.fill(0)
}

fun keyErrorMessage(error: KeyException): String = when (error) {
    is KeyException.PassphraseRequired -> "This key is encrypted. Enter its passphrase and retry."
    is KeyException.WrongPassphrase -> "Incorrect passphrase. Retry with the correct passphrase."
    is KeyException.UnsupportedFormat -> "Only OpenSSH private keys are supported. Convert PEM/PKCS#8 with ssh-keygen -p first."
    is KeyException.Malformed -> "The file is not a readable OpenSSH private key."
    is KeyException.UnsupportedAlgorithm -> "This key algorithm is not supported. Import Ed25519, ECDSA or RSA."
}

/** The label of a key made for a host by **New key** (Easy pair's review, the host form): `Key for Work Mac`. */
fun newKeyLabel(hostName: String) = "Key for ${hostName.trim()}"

/** Its public-key comment: `or2@<the phone's model>`. */
fun newKeyComment(device: String) = "or2@${device.trim()}"

/** What a screen says when its **New key** could not be made (the biometric was cancelled, the vault refused). */
fun newKeyErrorMessage(error: Throwable): String = when (error) {
    is KeyException -> keyErrorMessage(error)
    is VaultException, is GeneralSecurityException -> vaultErrorMessage(error)
    else -> "The key could not be created. Try again."
}
