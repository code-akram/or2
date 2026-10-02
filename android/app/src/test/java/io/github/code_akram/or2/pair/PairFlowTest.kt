package io.github.code_akram.or2.pair

import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.HostEndpoint
import io.github.code_akram.or2.data.KeyRecord
import io.github.code_akram.or2.ffi.PairCode
import io.github.code_akram.or2.ffi.PairException
import io.github.code_akram.or2.ffi.PairOffer
import io.github.code_akram.or2.ffi.PairResult
import io.github.code_akram.or2.ffi.PublicKeyInfo
import io.github.code_akram.or2.ffi.pairNewCode
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
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

private const val HOST_KEY = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAc39XUWT33SvSLy6vA7I83+XgmwnHmYtMQRjLeaZ2U7"
private const val HOST_FINGERPRINT = "SHA256:PK/nvGiFusFK9/6Qf8dSOX99mI5XQYiMAML2JHCjgQI"
private const val PAIRING_ID = "abcdefghijklm"

/** The text of a host's QR; a code made with --manual has no pairing id. */
private fun code(pairingId: String? = PAIRING_ID) =
    "or2-pair:2?name=Work%20Mac&user=alice&port=2222&a=100.101.102.103&a=work-mac.local" +
        "&hk=" + HOST_KEY.replace(" ", "%20").replace("+", "%2B") +
        if (pairingId != null) "&id=$pairingId" else ""

private fun keyRecord(id: String, fingerprint: String = "SHA256:$id") =
    KeyRecord(id, "Key $id", "ssh-ed25519", "ssh-ed25519 AAAA-$id", fingerprint, "", ByteArray(0), ByteArray(0))

/** What the backend saw of one enrolment: the key line, the device label, the offer's pairing id and the code `K`. */
private data class Enrolment(val keyLine: String, val device: String, val pairingId: String?, val code: String)

/** A backend whose enrolment is scripted; codes and the parser are the real native ones. */
private class FakeBackend : PairBackend {
    val events = mutableListOf<String>()
    val enrolments = mutableListOf<Enrolment>()
    var failure: PairException? = null
    var gate: CompletableDeferred<Unit>? = null

    override fun newCode(): PairCode = pairNewCode()
    override fun parse(text: String): PairOffer = parsePairPayload(text)

    override suspend fun enroll(offer: PairOffer, code: PairCode, publicKeyLine: String, deviceLabel: String): PairResult {
        events += "enroll"
        enrolments += Enrolment(publicKeyLine, deviceLabel, offer.pairingId, code.display())
        gate?.await()
        failure?.let { throw it }
        return PairResult(offer.username, "SHA256:installed")
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

    private fun shown() = (flow.state.value as PairState.Scanning).code.text

    private suspend fun awaitState(predicate: (PairState) -> Boolean) = withTimeout(5_000) { flow.state.first(predicate) }

    @Test
    fun theScreenShowsAPairingCodeInCrockfordWithoutZGroupsOfFourAndEveryStartDrawsANewOne() {
        val first = shown()
        assertTrue(first, Regex("[0-9A-HJKMNP-TV-Y]{4}-[0-9A-HJKMNP-TV-Y]{4}-[0-9A-HJKMNP-TV-Y]{4}").matches(first))
        flow.start()
        val second = shown()
        assertNotEquals(first, second)
        flow.start()
        assertNotEquals(second, shown())
    }

    @Test
    fun aValidCodeOpensTheReviewWithTheHostsOwnNameUserAndTheFirstKeyChosen() {
        assertTrue(flow.onCode(code(), keys))
        val review = review()
        assertEquals("Work Mac", review.name)
        assertEquals("alice", review.username)
        assertEquals(KeyChoice.Existing("a"), review.choice)
        assertEquals(listOf("100.101.102.103", "work-mac.local"), review.offer.addresses.map { it.host })
        assertEquals(HOST_FINGERPRINT, review.offer.hostKey.fingerprint)
        assertEquals(PAIRING_ID, review.offer.pairingId)
        assertTrue(review.enrolls)
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
    fun aBadCodeStaysOnTheScannerWithAReasonKeepsTheCodeAndAnotherCodeIsStillAccepted() {
        val shown = shown()
        for ((text, fragment) in listOf(
            "hello" to "not an or2 pairing code",
            "or2-pair:1?name=x" to "older or2-pair: update it on the host",
            "or2-pair:3?name=x" to "Update or2 to use this code",
            "or2-pair:2?name=x" to "not valid",
            "or2-pair:2?%zz" to "damaged",
        )) {
            assertFalse(text, flow.onCode(text, keys))
            val state = flow.state.value as PairState.Scanning
            assertTrue("$text: ${state.error}", state.error!!.contains(fragment))
            assertEquals(shown, state.code.text)
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
    fun theUserOfACodeWithAPairingIdIsTheHostsAccountAndCannotBeEdited() {
        // The host authorizes the key for the account that ran or2-pair; a different login here would
        // pair one account and then connect as another.
        flow.onCode(code(), keys)
        assertTrue(review().enrolls)
        flow.edit(username = "bob")
        assertEquals("alice", review().username)
        // A --manual code only describes the host: the user may name the login to install the key for.
        flow.start()
        flow.onCode(code(pairingId = null), keys)
        assertFalse(review().enrolls)
        flow.edit(username = "bob")
        assertEquals("bob", review().username)
    }

    @Test
    fun theHostIsSavedWithItsKeyTrustedOnlyAfterTheHostInstalledTheKey() = runBlocking<Unit> {
        val typedOnTheHost = shown()
        flow.onCode(code(), keys)
        flow.edit(choice = KeyChoice.Existing("b"))
        backend.gate = CompletableDeferred()
        flow.submit(keys, "Pixel 8", generate)

        // Mid-pairing: the host is being talked to, nothing is stored.
        assertEquals("Work Mac", (flow.state.value as PairState.Pairing).review.name)
        assertEquals(listOf("enroll"), backend.events)
        // The code the user typed on the host is the one the key was derived from.
        assertEquals(Enrolment("ssh-ed25519 AAAA-b", "Pixel 8", PAIRING_ID, typedOnTheHost), backend.enrolments.single())
        assertTrue(store.saved.isEmpty())

        backend.gate!!.complete(Unit)
        val paired = awaitState { it is PairState.Paired } as PairState.Paired
        assertEquals(listOf("enroll", "save"), backend.events)
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
    fun theSpentCodeIsReplacedOnceTheHostInstalledTheKey() = runBlocking<Unit> {
        val spent = shown()
        flow.onCode(code(), keys)
        flow.submit(keys, "Pixel", generate)
        awaitState { it is PairState.Paired }
        assertNotNull(flow.consume())
        assertNotEquals(spent, shown())
    }

    @Test
    fun aNewKeyIsGeneratedFirstThenAuthorizedThenTheHostIsSavedWithIt() = runBlocking<Unit> {
        flow.onCode(code(), emptyList())
        flow.edit(name = "My Mac")
        flow.submit(emptyList(), "Pixel 8", generate)
        awaitState { it is PairState.Paired }
        assertEquals(listOf("generate", "enroll", "save"), backend.events)
        assertEquals(listOf("Key for My Mac" to "or2@Pixel 8"), generated)
        assertEquals("ssh-ed25519 AAAA-new", backend.enrolments.single().keyLine)
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
    fun aFailureBeforeTheHostSawTheCodeReturnsToTheReviewWithTheSameCodeAndSavesNothing() = runBlocking<Unit> {
        for ((failure, fragment) in listOf(
            PairException.Unreachable() to "Couldn't reach Work Mac on port 2222",
            PairException.HostKeyMismatch() to "different key than the code",
            PairException.InvalidKey() to "cannot be sent",
            PairException.InvalidDevice() to "name cannot be sent",
            PairException.InvalidOffer() to "cannot be used",
            PairException.NoPairingId() to "set up by hand",
        )) {
            backend.failure = failure
            flow.start()
            val typedOnTheHost = shown()
            flow.onCode(code(), keys)
            flow.submit(keys, "Pixel", generate)
            val review = (awaitState { it is PairState.Review && !it.review.working } as PairState.Review).review
            assertTrue("${failure::class.simpleName}: ${review.error}", review.error!!.contains(fragment))
            assertFalse(reachedHost(failure))
            assertTrue(store.saved.isEmpty())
            // Back to scanning from the review keeps the code: the host may still be waiting for it.
            flow.rescan()
            assertEquals(typedOnTheHost, shown())
        }
    }

    @Test
    fun aFailureAfterTheHostSawTheCodeReturnsToScanningWithItsReasonAndANewCode() = runBlocking<Unit> {
        for ((failure, fragment) in listOf(
            PairException.BootstrapRefused() to "didn't accept the pairing key",
            PairException.NotOr2Pair() to "Something other than or2-pair answered",
            PairException.Expired() to "has stopped or timed out",
            PairException.Gone() to "Another device already used this pairing",
            PairException.KeyNotAccepted() to "couldn't add the key",
            PairException.HostFailed() to "couldn't add the key",
            PairException.TimedOut() to "did not answer in time",
            PairException.ConnectionLost() to "ended early",
            PairException.Protocol() to "did not understand",
            PairException.Refused() to "refused",
        )) {
            backend.failure = failure
            flow.start()
            val spent = shown()
            flow.onCode(code(), keys)
            flow.submit(keys, "Pixel", generate)
            val state = awaitState { it is PairState.Scanning && it.code.text != spent } as PairState.Scanning
            assertTrue("${failure::class.simpleName}: ${state.error}", state.error!!.contains(fragment))
            assertTrue(reachedHost(failure))
            assertTrue(store.saved.isEmpty())
        }
    }

    @Test
    fun aFailedPairingThatDidNotReachTheHostCanBeRetriedWithTheSameCode() = runBlocking<Unit> {
        backend.failure = PairException.Unreachable()
        flow.onCode(code(), keys)
        flow.submit(keys, "Pixel", generate)
        awaitState { it is PairState.Review && !it.review.working && it.review.error != null }
        backend.failure = null
        flow.submit(keys, "Pixel", generate)
        awaitState { it is PairState.Paired }
        assertEquals(2, backend.enrolments.size)
        assertEquals(backend.enrolments[0].code, backend.enrolments[1].code)
        assertEquals(1, store.saved.size)
    }

    @Test
    fun ifSavingFailsAfterTheHostInstalledTheKeyTheRetrySavesWithoutPairingAgain() = runBlocking<Unit> {
        store.failures = 1
        flow.onCode(code(), keys)
        flow.submit(keys, "Pixel", generate)
        val review = (awaitState { it is PairState.Review && !it.review.working } as PairState.Review).review
        assertTrue(review.error!!.contains("accepted the key"))
        assertEquals(AcceptedKey("a", "SHA256:a"), review.accepted)
        flow.submit(keys, "Pixel", generate)
        awaitState { it is PairState.Paired }
        assertEquals("the spent code is not used twice", 1, backend.enrolments.size)
        assertEquals(listOf("enroll", "save", "save"), backend.events)
    }

    @Test
    fun afterASaveFailureTheKeyCannotBeChangedAndOnlyTheSaveIsRepeated() = runBlocking<Unit> {
        // Review of the v2 integration: changing the key after a save failure started a second enrolment. Once
        // the host accepted key a, the retry is bound to a, the key cannot be changed, and only the save is repeated.
        store.failures = 1
        flow.onCode(code(), keys)
        flow.submit(keys, "Pixel", generate)
        awaitState { it is PairState.Review && !it.review.working && it.review.accepted != null }
        assertTrue(review().keyLocked)
        flow.edit(choice = KeyChoice.Existing("b"))
        assertEquals(KeyChoice.Existing("a"), review().choice)
        flow.edit(choice = KeyChoice.New)
        assertEquals(KeyChoice.Existing("a"), review().choice)
        // The name stays editable: it is only what the phone saves.
        flow.edit(name = "Office")
        assertEquals("Office", review().name)
        flow.submit(keys, "Pixel", generate)
        val paired = awaitState { it is PairState.Paired } as PairState.Paired
        assertEquals("no second enrolment", 1, backend.enrolments.size)
        assertEquals("ssh-ed25519 AAAA-a", backend.enrolments.single().keyLine)
        assertEquals(listOf("enroll", "save", "save"), backend.events)
        assertEquals("a", store.saved.single().first.keyId)
        assertEquals("Office", paired.host.label)
    }

    @Test
    fun aNewKeyTheHostAcceptedIsTheRetrysKeyAndIsNotGeneratedAgain() = runBlocking<Unit> {
        store.failures = 1
        flow.onCode(code(), emptyList())
        flow.submit(emptyList(), "Pixel", generate)
        val failed = (awaitState { it is PairState.Review && !it.review.working } as PairState.Review).review
        assertEquals(AcceptedKey("new", "SHA256:new"), failed.accepted)
        assertEquals(KeyChoice.Existing("new"), failed.choice)
        flow.edit(choice = KeyChoice.New)
        assertEquals(KeyChoice.Existing("new"), review().choice)
        // The key list the screen passes now holds the generated key.
        val stored = listOf(newKey) + keys
        flow.submit(stored, "Pixel", generate)
        awaitState { it is PairState.Paired }
        assertEquals(listOf("generate", "enroll", "save", "save"), backend.events)
        assertEquals(1, generated.size)
        assertEquals("new", store.saved.single().first.keyId)
    }

    @Test
    fun aRetryWhoseAcceptedKeyChangedOrWentAwayNeverEnrolsAgain() = runBlocking<Unit> {
        for (changed in listOf(listOf(keyRecord("a", fingerprint = "SHA256:other"), keyRecord("b")), listOf(keyRecord("b")))) {
            store.failures = 1
            backend.enrolments.clear()
            backend.events.clear()
            flow.start()
            flow.onCode(code(), keys)
            flow.submit(keys, "Pixel", generate)
            awaitState { it is PairState.Review && !it.review.working && it.review.accepted != null }
            // Key a is no longer the key the host accepted (deleted, or another key under its id): the host's run is
            // spent, so the screen starts over with a new code instead of enrolling or saving the wrong key.
            flow.submit(changed, "Pixel", generate)
            val state = awaitState { it is PairState.Scanning } as PairState.Scanning
            assertTrue(state.error!!, state.error!!.contains("Run or2-pair again"))
            assertEquals(1, backend.enrolments.size)
            assertEquals(listOf("enroll", "save"), backend.events)
            assertTrue(store.saved.isEmpty())
        }
    }

    @Test
    fun aSecondTapWhilePairingDoesNothing() = runBlocking<Unit> {
        backend.gate = CompletableDeferred()
        flow.onCode(code(), keys)
        flow.submit(keys, "Pixel", generate)
        flow.submit(keys, "Pixel", generate)
        flow.edit(name = "ignored")
        assertEquals(1, backend.enrolments.size)
        assertEquals("Work Mac", (flow.state.value as PairState.Pairing).review.name)
        backend.gate!!.complete(Unit)
        awaitState { it is PairState.Paired }
        assertEquals(1, backend.enrolments.size)
    }

    @Test
    fun aManualCodeSavesTheHostAndShowsTheKeyToInstallWithoutPairing() = runBlocking<Unit> {
        flow.onCode(code(pairingId = null), emptyList())
        flow.submit(emptyList(), "Pixel", generate)
        val state = awaitState { it is PairState.KeyToInstall } as PairState.KeyToInstall
        assertEquals(listOf("generate", "save"), backend.events)
        assertTrue(backend.enrolments.isEmpty())
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
    fun cancellingWhilePairingAbortsAndDropsTheCode() = runBlocking<Unit> {
        backend.gate = CompletableDeferred()
        val typedOnTheHost = shown()
        flow.onCode(code(), keys)
        flow.submit(keys, "Pixel", generate)
        assertTrue(flow.state.value is PairState.Pairing)
        flow.cancel()
        assertTrue(flow.state.value is PairState.Scanning)
        assertNotEquals(typedOnTheHost, shown())
        backend.gate!!.complete(Unit)
        assertTrue(store.saved.isEmpty())
        assertTrue(flow.state.value is PairState.Scanning)
    }

    @Test
    fun aFailureThatArrivesAfterCancellingDoesNotOverwriteTheScreen() = runBlocking<Unit> {
        backend.gate = CompletableDeferred()
        backend.failure = PairException.BootstrapRefused()
        flow.onCode(code(), keys)
        flow.submit(keys, "Pixel", generate)
        flow.cancel()
        val after = flow.state.value
        backend.gate!!.complete(Unit)
        assertEquals(after, flow.state.value)
        assertNull((flow.state.value as PairState.Scanning).error)
    }

    @Test
    fun leavingTheReviewKeepsTheCodeAndStartingOverDrawsANewOne() {
        val first = shown()
        flow.onCode(code(), keys)
        flow.rescan()
        assertTrue(flow.state.value is PairState.Scanning)
        assertEquals(first, shown())
        flow.start()
        assertNotEquals(first, shown())
    }

    @Test
    fun noStateOfTheFlowPrintsTheCodeOrTheQr() {
        val typed = shown()
        assertFalse(flow.state.value.toString(), flow.state.value.toString().contains(typed))
        flow.onCode(code(), keys)
        val text = flow.state.value.toString()
        assertFalse(text, text.contains(typed))
    }
}
