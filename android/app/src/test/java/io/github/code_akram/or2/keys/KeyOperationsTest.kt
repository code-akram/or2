package io.github.code_akram.or2.keys

import io.github.code_akram.or2.ffi.generateEd25519Key
import io.github.code_akram.or2.ffi.KeyException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.ExperimentalCoroutinesApi
import org.junit.Assert.*
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.IOException
import java.io.InputStream
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.spec.GCMParameterSpec

@OptIn(ExperimentalCoroutinesApi::class)
class KeyOperationsTest {
    @Test
    fun aFingerprintIsEllipsizedInTheMiddleForListRows() {
        assertEquals("SHA256:7vK2mQ9x…tB1MkA", shortFingerprint("SHA256:7vK2mQ9xRpL3aTn0sWZ4cEdHfY8uJbNqXoGiVtB1MkA"))
        assertEquals("SHA256:short", shortFingerprint("SHA256:short"))
        assertEquals("no-prefix", shortFingerprint("no-prefix"))
        assertEquals("abcdefgh…uvwxyz", shortFingerprint("abcdefghijklmnopqrstuvwxyz"))
    }

    @Test
    fun importedInputIsWipedOnSuccessAndTypedFailure() {
        val generated = generateEd25519Key("")
        try {
            val bytes = generated.privateKey.copyOf()
            val imported = importAndWipe(bytes, null)
            try {
                assertArrayEquals(ByteArray(bytes.size), bytes)
                assertArrayEquals(generated.privateKey, imported.privateKey)
            } finally { imported.privateKey.fill(0) }
            val invalid = byteArrayOf(7, 3, 9)
            assertThrows(KeyException.Malformed::class.java) { importAndWipe(invalid, null) }
            assertArrayEquals(ByteArray(3), invalid)
        } finally { generated.privateKey.fill(0) }
    }

    @Test
    fun encryptionUsesRealMaterialThenWipesAndPreservesMetadata() = runTest {
        val material = generateEd25519Key("")
        val expected = material.privateKey.copyOf()
        try {
            val key = KeyGenerator.getInstance("AES").apply { init(256) }.generateKey()
            val cipher = Cipher.getInstance("AES/GCM/NoPadding").apply { init(Cipher.ENCRYPT_MODE, key) }
            val encrypted = encryptKey("ephemeral", "Label", material) {
                assertArrayEquals(expected, material.privateKey)
                cipher
            }
            assertArrayEquals(ByteArray(expected.size), material.privateKey)
            assertEquals(material.publicKey.openssh, encrypted.openssh)
            assertEquals(material.publicKey.fingerprint, encrypted.fingerprint)
            val plain = Cipher.getInstance("AES/GCM/NoPadding").run {
                init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, encrypted.iv))
                doFinal(encrypted.ciphertext)
            }
            try { assertArrayEquals(expected, plain) } finally { plain.fill(0) }
        } finally { expected.fill(0); material.privateKey.fill(0) }
    }

    @Test
    fun promptFailureAndCancellationWipeReturnedMaterial() = runTest {
        val failed = generateEd25519Key("")
        try { encryptKey("ephemeral", "Label", failed) { error("prompt failed") }; fail("Expected failure") }
        catch (_: IllegalStateException) { }
        assertTrue(failed.privateKey.all { it == 0.toByte() })
        val cancelled = generateEd25519Key("")
        val gate = CompletableDeferred<Cipher>()
        val job = launch { encryptKey("ephemeral", "Label", cancelled) { gate.await() } }
        runCurrent()
        assertTrue(cancelled.privateKey.any { it != 0.toByte() })
        job.cancel()
        job.join()
        assertTrue(cancelled.privateKey.all { it == 0.toByte() })
    }

    @Test
    fun fileReadsAreBoundedAndWipeStagingOnReadOrCloseFailure() {
        val expected = byteArrayOf(3, 1, 8)
        val bytes = readPrivateKey(ByteArrayInputStream(expected))
        try { assertArrayEquals(expected, bytes) } finally { bytes.fill(0); expected.fill(0) }
        assertThrows(IllegalArgumentException::class.java) {
            readPrivateKey(ByteArrayInputStream(ByteArray(1024 * 1024 + 1)))
        }
        for (closeFailure in listOf(false, true)) {
            var staging: ByteArray? = null
            val input = object : InputStream() {
                var first = true
                override fun read(): Int = -1
                override fun read(b: ByteArray, off: Int, len: Int): Int {
                    staging = b
                    if (first) { first = false; b[off] = 17; return 1 }
                    if (!closeFailure) throw IOException("read failed")
                    return -1
                }
                override fun close() { if (closeFailure) throw IOException("close failed") }
            }
            assertThrows(IOException::class.java) { readPrivateKey(input) }
            assertTrue(staging!!.all { it == 0.toByte() })
        }
    }
}
