package io.github.code_akram.or2.keys

import io.github.code_akram.or2.data.KeyRecord
import io.github.code_akram.or2.ffi.ClientKeyMaterial
import io.github.code_akram.or2.ffi.KeyException
import io.github.code_akram.or2.ffi.importPrivateKey
import java.io.InputStream
import javax.crypto.Cipher

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
