package io.github.code_akram.or2.pair

import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.HostEndpoint
import io.github.code_akram.or2.data.KeyRecord
import io.github.code_akram.or2.ffi.PairException
import io.github.code_akram.or2.ffi.PairOffer
import io.github.code_akram.or2.ffi.PublicKeyInfo
import io.github.code_akram.or2.ffi.parsePairPayload
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

private const val HOST_KEY = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAc39XUWT33SvSLy6vA7I83+XgmwnHmYtMQRjLeaZ2U7"
private const val HOST_FINGERPRINT = "SHA256:PK/nvGiFusFK9/6Qf8dSOX99mI5XQYiMAML2JHCjgQI"
private const val OTP = "AAAQEAYEAUDAOCAJBIFQYDIOB4"

private fun code(listens: Boolean = true) =
    "or2-pair:1?name=Work%20Mac&user=alice&port=2222&a=100.101.102.103&a=work-mac.local" +
        "&hk=" + HOST_KEY.replace(" ", "%20").replace("+", "%2B") +
        if (listens) "&pair=192.168.1.20:41234&otp=$OTP" else ""

private fun keyRecord(id: String, fingerprint: String = "SHA256:$id") =
    KeyRecord(id, "Key $id", "ssh-ed25519", "ssh-ed25519 AAAA-$id", fingerprint, "", ByteArray(0), ByteArray(0))

/** A backend whose exchange is scripted and whose parser is the real native one. */
private class FakeBackend : PairBackend {
    val events = mutableListOf<String>()
    val submitted = mutableListOf<Triple<String, String, Boolean>>() // key line, device, secret still intact
    var failure: PairException? = null
    var gate: CompletableDeferred<Unit>? = null

    override fun parse(text: String): PairOffer = parsePairPayload(text)

    override suspend fun submit(offer: PairOffer, publicKeyLine: String, deviceLabel: String) {
        events += "submit"
        submitted += Triple(publicKeyLine, deviceLabel, offer.exchange?.secret?.isWiped() == false)
        gate?.await()
        failure?.let { throw it }
    }
}

private class FakeStore(val events: MutableList<String>) : PairStore {
    val saved = mutableListOf<Pair<Host, PublicKeyInfo>>()
    var failures = 0
    override suspend fun saveTrustedHost(host: Host, hostKey: PublicKeyInfo): Host {
        events += "save"
        if (failures > 0) {
            failures--
            error("disk full")
        }
        saved += host to hostKey
        return host.copy(record = host.record.copy(id = 41))
    }
}

class PairFlowTest {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
    private val backend = FakeBackend()
    private val store = FakeStore(backend.events)
    private val keys = listOf(keyRecord("a"), keyRecord("b"))
    private val generated = mutableListOf<Pair<String, String>>()
    private val newKey = keyRecord("new")
    private var generateFailure: Exception? = null
    private val flow = PairFlow(backend, store, scope) { "key error: ${it.message}" }

    private val generate: suspend (String, String) -> KeyRecord = { label, comment ->
        backend.events += "generate"
        generated += label to comment
        generateFailure?.let { throw it }
        newKey
    }

    @After
    fun stop() {
        scope.cancel()
    }

    private fun review() = (flow.state.value as PairState.Review).review

    private suspend fun awaitState(predicate: (PairState) -> Boolean) = withTimeout(5_000) { flow.state.first(predicate) }

    @Test
    fun aValidCodeOpensTheReviewWithTheHostsOwnNameUserAndTheFirstKeyChosen() {
        assertTrue(flow.state.value is PairState.Scanning)
        assertTrue(flow.onCode(code(), keys))
        val review = review()
        assertEquals("Work Mac", review.name)
        assertEquals("alice", review.username)
        assertEquals(KeyChoice.Existing("a"), review.choice)
        assertEquals(listOf("100.101.102.103", "work-mac.local"), review.offer.addresses.map { it.host })
        assertEquals(HOST_FINGERPRINT, review.offer.hostKey.fingerprint)
        assertTrue(review.listens)
        assertTrue(review.valid)
        assertNull(review.error)
    }

    @Test
    fun withNoStoredKeyANewOneIsTheDefault() {
        flow.onCode(code(), emptyList())
        assertEquals(KeyChoice.New, review().choice)
        assertEquals(KeyChoice.New, defaultKeyChoice(emptyList()))
        assertEquals(KeyChoice.Existing("a"), defaultKeyChoice(keys))
    }

    @Test
    fun aBadCodeStaysOnTheScannerWithAReasonAndAnotherCodeIsStillAccepted() {
        for ((text, fragment) in listOf(
            "hello" to "not an or2 pairing code",
            "or2-pair:2?name=x" to "newer or2-pair",
            "or2-pair:1?name=x" to "not valid",
            "or2-pair:1?%zz" to "damaged",
        )) {
            assertFalse(text, flow.onCode(text, keys))
            val state = flow.state.value as PairState.Scanning
            assertTrue("$text: ${state.error}", state.error!!.contains(fragment))
        }
        assertTrue(flow.onCode(code(), keys))
    }

    @Test
    fun onceTheReviewIsOpenFurtherScansAreIgnored() {
        flow.onCode(code(), keys)
        flow.edit(name = "Mine")
        // The camera keeps decoding frames: a second code must not replace what the user is reviewing.
        assertFalse(flow.onCode(code().replace("Work%20Mac", "Other"), keys))
        assertEquals("Mine", review().name)
    }

    @Test
    fun editingChangesFieldsAndClearsTheLastError() {
        flow.onCode(code(), keys)
        flow.edit(name = "Home", choice = KeyChoice.New)
        assertEquals(Triple("Home", "alice", KeyChoice.New), review().let { Triple(it.name, it.username, it.choice) })
        flow.edit(name = "   ")
        assertFalse(review().valid)
        flow.submit(keys, "Pixel", generate)
        assertEquals("Enter a name and a user name for this host.", review().error)
        assertTrue(backend.events.isEmpty())
        flow.edit(name = "Home")
        assertNull(review().error)
    }

    @Test
    fun theUserOfACodeWithAListenerIsTheHostsAccountAndCannotBeEdited() {
        // The host authorizes the key for the account that ran or2-pair; a different login here would
        // pair one account and then connect as another.
        flow.onCode(code(listens = true), keys)
        assertTrue(review().listens)
        flow.edit(username = "bob")
        assertEquals("alice", review().username)
        // A code without a listener only describes the host: the user may name the login to install the key for.
        flow.start()
        flow.onCode(code(listens = false), keys)
        flow.edit(username = "bob")
        assertEquals("bob", review().username)
    }

    @Test
    fun theHostIsSavedWithItsKeyTrustedOnlyAfterTheHostAcceptedTheKey() = runBlocking<Unit> {
        flow.onCode(code(), keys)
        flow.edit(choice = KeyChoice.Existing("b"))
        backend.gate = CompletableDeferred()
        flow.submit(keys, "Pixel 8", generate)

        // Mid-exchange: the host is being asked, nothing is stored, the fingerprint to compare is on screen.
        val submitting = flow.state.value as PairState.Submitting
        assertEquals("SHA256:b", submitting.phoneFingerprint)
        assertEquals(listOf("submit"), backend.events)
        assertEquals(Triple("ssh-ed25519 AAAA-b", "Pixel 8", true), backend.submitted.single())
        assertTrue(store.saved.isEmpty())

        backend.gate!!.complete(Unit)
        val paired = awaitState { it is PairState.Paired } as PairState.Paired
        assertEquals(listOf("submit", "save"), backend.events)
        val (host, trusted) = store.saved.single()
        assertEquals("Work Mac", host.label)
        assertEquals("alice", host.username)
        assertEquals("b", host.keyId)
        assertEquals(listOf(HostEndpoint("100.101.102.103", 2222), HostEndpoint("work-mac.local", 2222)), host.addresses)
        // The host key of the code is what gets trusted: the same fingerprint the user was shown.
        assertEquals(HOST_FINGERPRINT, trusted.fingerprint)
        assertEquals(41, paired.host.id)
    }

    @Test
    fun thePasswordIsWipedOnceTheHostIsSaved() = runBlocking<Unit> {
        flow.onCode(code(), keys)
        val secret = review().offer.exchange!!.secret
        assertFalse(secret.isWiped())
        flow.submit(keys, "Pixel", generate)
        awaitState { it is PairState.Paired }
        assertTrue(secret.isWiped())
    }

    @Test
    fun aNewKeyIsGeneratedFirstThenAuthorizedThenTheHostIsSavedWithIt() = runBlocking<Unit> {
        flow.onCode(code(), emptyList())
        flow.edit(name = "My Mac")
        flow.submit(emptyList(), "Pixel 8", generate)
        awaitState { it is PairState.Paired }
        assertEquals(listOf("generate", "submit", "save"), backend.events)
        assertEquals(listOf("Key for My Mac" to "or2@Pixel 8"), generated)
        assertEquals("ssh-ed25519 AAAA-new", backend.submitted.single().first)
        assertEquals("new", store.saved.single().first.keyId)
    }

    @Test
    fun aKeyThatCouldNotBeMadeIsReportedAndNothingIsSentOrSaved() = runBlocking<Unit> {
        generateFailure = IllegalStateException("biometric cancelled")
        flow.onCode(code(), emptyList())
        flow.submit(emptyList(), "Pixel", generate)
        val review = (awaitState { it is PairState.Review && !it.review.working } as PairState.Review).review
        assertEquals("key error: biometric cancelled", review.error)
        assertEquals(listOf("generate"), backend.events)
        assertFalse(review.offer.exchange!!.secret.isWiped())
        // Trying again works.
        generateFailure = null
        flow.submit(emptyList(), "Pixel", generate)
        awaitState { it is PairState.Paired }
    }

    @Test
    fun aKeyDeletedWhileReviewingIsReported() = runBlocking<Unit> {
        flow.onCode(code(), keys)
        flow.edit(choice = KeyChoice.Existing("gone"))
        flow.submit(keys, "Pixel", generate)
        assertEquals("That key was deleted. Choose another.", review().error)
        assertTrue(backend.events.isEmpty())
    }

    @Test
    fun everyRefusalReturnsToTheReviewWithItsOwnWordsAndSavesNothing() = runBlocking<Unit> {
        for ((failure, fragment) in listOf(
            PairException.Declined() to "declined",
            PairException.AuthenticationFailed() to "already have been used",
            PairException.HostTimedOut() to "Nobody confirmed",
            PairException.TimedOut() to "Nobody confirmed",
            PairException.Unreachable() to "same network",
            PairException.ConnectionLost() to "ended early",
            PairException.Protocol() to "did not understand",
            PairException.KeyNotAccepted() to "kind of key",
            PairException.Wiped() to "already used",
        )) {
            backend.failure = failure
            flow.start()
            flow.onCode(code(), keys)
            flow.submit(keys, "Pixel", generate)
            val review = (awaitState { it is PairState.Review && !it.review.working } as PairState.Review).review
            assertTrue("${failure::class.simpleName}: ${review.error}", review.error!!.contains(fragment))
            assertTrue(store.saved.isEmpty())
        }
    }

    @Test
    fun aFailedExchangeCanBeRetriedWithTheSameCode() = runBlocking<Unit> {
        backend.failure = PairException.Unreachable()
        flow.onCode(code(), keys)
        flow.submit(keys, "Pixel", generate)
        awaitState { it is PairState.Review && !it.review.working && it.review.error != null }
        backend.failure = null
        flow.submit(keys, "Pixel", generate)
        awaitState { it is PairState.Paired }
        assertEquals(2, backend.submitted.size)
        assertEquals(1, store.saved.size)
    }

    @Test
    fun ifSavingFailsAfterTheHostAcceptedTheKeyTheRetrySavesWithoutExchangingAgain() = runBlocking<Unit> {
        store.failures = 1
        flow.onCode(code(), keys)
        flow.submit(keys, "Pixel", generate)
        val review = (awaitState { it is PairState.Review && !it.review.working } as PairState.Review).review
        assertTrue(review.error!!.contains("accepted the key"))
        assertEquals("SHA256:a", review.acceptedKey)
        flow.submit(keys, "Pixel", generate)
        awaitState { it is PairState.Paired }
        assertEquals("the spent code is not used twice", 1, backend.submitted.size)
        assertEquals(listOf("submit", "save", "save"), backend.events)
    }

    @Test
    fun aSecondTapWhileWorkingDoesNothing() = runBlocking<Unit> {
        backend.gate = CompletableDeferred()
        flow.onCode(code(), keys)
        flow.submit(keys, "Pixel", generate)
        flow.submit(keys, "Pixel", generate)
        flow.edit(name = "ignored")
        assertEquals(1, backend.submitted.size)
        assertEquals("Work Mac", (flow.state.value as PairState.Submitting).review.name)
        backend.gate!!.complete(Unit)
        awaitState { it is PairState.Paired }
        assertEquals(1, backend.submitted.size)
    }

    @Test
    fun aCodeWithoutAListenerSavesTheHostAndShowsTheKeyToInstall() = runBlocking<Unit> {
        flow.onCode(code(listens = false), emptyList())
        assertFalse(review().listens)
        flow.submit(emptyList(), "Pixel", generate)
        val state = awaitState { it is PairState.KeyToInstall } as PairState.KeyToInstall
        assertEquals(listOf("generate", "save"), backend.events)
        assertEquals("ssh-ed25519 AAAA-new", state.keyLine)
        assertEquals("SHA256:new", state.fingerprint)
        assertEquals(HOST_FINGERPRINT, store.saved.single().second.fingerprint)
        assertNotNull(flow.consume())
        assertTrue(flow.state.value is PairState.Scanning)
    }

    @Test
    fun consumingThePairedHostReturnsToScanningOnce() = runBlocking<Unit> {
        flow.onCode(code(), keys)
        flow.submit(keys, "Pixel", generate)
        awaitState { it is PairState.Paired }
        assertEquals(41, flow.consume()!!.id)
        assertNull(flow.consume())
        assertTrue(flow.state.value is PairState.Scanning)
    }

    @Test
    fun cancellingWhileTheHostIsBeingAskedAbortsAndWipesTheCode() = runBlocking<Unit> {
        backend.gate = CompletableDeferred()
        flow.onCode(code(), keys)
        val secret = review().offer.exchange!!.secret
        flow.submit(keys, "Pixel", generate)
        assertTrue(flow.state.value is PairState.Submitting)
        flow.cancel()
        assertTrue(flow.state.value is PairState.Scanning)
        assertTrue(secret.isWiped())
        backend.gate!!.complete(Unit)
        assertTrue(store.saved.isEmpty())
        assertTrue(flow.state.value is PairState.Scanning)
    }

    @Test
    fun startingOverAndLeavingTheReviewWipeTheCode() {
        flow.onCode(code(), keys)
        val first = review().offer.exchange!!.secret
        flow.rescan()
        assertTrue(first.isWiped())
        assertTrue(flow.state.value is PairState.Scanning)
        flow.onCode(code(), keys)
        val second = review().offer.exchange!!.secret
        flow.start()
        assertTrue(second.isWiped())
    }

    @Test
    fun noStateOfTheFlowPrintsThePasswordOrTheCode() {
        flow.onCode(code(), keys)
        val shown = flow.state.value.toString()
        assertFalse(shown, shown.contains(OTP))
        assertFalse(shown, shown.contains("otp="))
    }
}
