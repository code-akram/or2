package io.github.code_akram.or2

import io.github.code_akram.or2.ffi.KeyException
import io.github.code_akram.or2.ffi.generateEd25519Key
import io.github.code_akram.or2.ffi.importPrivateKey
import java.io.File
import java.nio.file.Files
import java.nio.file.attribute.PosixFilePermissions
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

/** Key generation and import through the real library, checked against OpenSSH's ssh-keygen. */
class KeyContractTest {
    @get:Rule
    val temp = TemporaryFolder()

    @Before
    fun requireSshKeygen() {
        val path = System.getenv("PATH").orEmpty().split(File.pathSeparator)
        assumeTrue("ssh-keygen not on PATH", path.any { File(it, "ssh-keygen").canExecute() })
    }

    private fun sshKeygen(vararg args: String): String {
        val process = ProcessBuilder("ssh-keygen", *args).redirectErrorStream(true).start()
        val output = process.inputStream.bufferedReader().readText()
        assertEquals(output, 0, process.waitFor())
        return output.trim()
    }

    private fun privateFile(name: String, bytes: ByteArray): File {
        val file = temp.root.resolve(name)
        Files.write(file.toPath(), bytes)
        Files.setPosixFilePermissions(file.toPath(), PosixFilePermissions.fromString("rw-------"))
        return file
    }

    /** ssh-keygen -l prints `<bits> SHA256:<hash> <comment> (<type>)`. */
    private fun fingerprintOf(file: File) = sshKeygen("-l", "-E", "sha256", "-f", file.path).split(" ")[1]

    private fun newOpenSshKey(vararg args: String): File {
        val file = temp.root.resolve("id_${System.nanoTime()}")
        sshKeygen("-q", *args, "-f", file.path)
        return file
    }

    @Test
    fun generatedKeyIsReadableByOpenSsh() {
        val key = generateEd25519Key("or2@test")
        assertEquals("ssh-ed25519", key.publicKey.algorithm)
        assertEquals("or2@test", key.publicKey.comment)
        val text = key.privateKey.decodeToString()
        assertTrue(text.startsWith("-----BEGIN OPENSSH PRIVATE KEY-----\n"))
        assertFalse(text.contains('\r'))

        val private = privateFile("generated", key.privateKey)
        val derived = sshKeygen("-y", "-f", private.path).split(" ")
        assertEquals(key.publicKey.openssh.split(" ").take(2), derived.take(2))
        assertEquals(fingerprintOf(private), key.publicKey.fingerprint)
    }

    @Test
    fun encryptedOpenSshKeyImportsOnlyWithItsPassphrase() {
        val file = newOpenSshKey("-t", "ed25519", "-N", "correct horse", "-C", "imported")
        val bytes = file.readBytes()
        assertThrows(KeyException.PassphraseRequired::class.java) { importPrivateKey(bytes, null) }
        assertThrows(KeyException.WrongPassphrase::class.java) { importPrivateKey(bytes, "wrong horse") }

        val imported = importPrivateKey(bytes, "correct horse")
        assertEquals(fingerprintOf(file), imported.publicKey.fingerprint)
        assertEquals("imported", imported.publicKey.comment)
        // The storage form is decrypted: it re-imports without a passphrase and OpenSSH reads it.
        val again = importPrivateKey(imported.privateKey, null)
        assertEquals(imported.publicKey, again.publicKey)
        assertArrayEquals(imported.privateKey, again.privateKey)
        assertEquals(fingerprintOf(file), fingerprintOf(privateFile("stored", imported.privateKey)))
    }

    @Test
    fun rsaAndEcdsaKeysImport() {
        val rsa = newOpenSshKey("-t", "rsa", "-b", "3072", "-N", "")
        assertEquals("ssh-rsa", importPrivateKey(rsa.readBytes(), null).publicKey.algorithm)
        assertEquals(fingerprintOf(rsa), importPrivateKey(rsa.readBytes(), null).publicKey.fingerprint)
        val ecdsa = newOpenSshKey("-t", "ecdsa", "-b", "384", "-N", "")
        assertEquals(
            "ecdsa-sha2-nistp384",
            importPrivateKey(ecdsa.readBytes(), null).publicKey.algorithm,
        )
    }

    @Test
    fun nonOpenSshFormatsAndGarbageAreTypedErrors() {
        val pem = newOpenSshKey("-t", "rsa", "-b", "2048", "-m", "PEM", "-N", "")
        assertTrue(pem.readText().startsWith("-----BEGIN RSA PRIVATE KEY-----"))
        assertThrows(KeyException.UnsupportedFormat::class.java) { importPrivateKey(pem.readBytes(), null) }
        assertThrows(KeyException.Malformed::class.java) { importPrivateKey("hello".encodeToByteArray(), null) }
        val stored = generateEd25519Key("t").privateKey
        assertThrows(KeyException.Malformed::class.java) {
            importPrivateKey(stored.copyOf(stored.size / 2), null)
        }
    }
}
