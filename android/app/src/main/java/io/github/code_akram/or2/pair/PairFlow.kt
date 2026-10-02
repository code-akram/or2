package io.github.code_akram.or2.pair

import io.github.code_akram.or2.data.AppDao
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.HostEndpoint
import io.github.code_akram.or2.data.HostRecord
import io.github.code_akram.or2.data.KeyRecord
import io.github.code_akram.or2.ffi.PairCode
import io.github.code_akram.or2.ffi.PairException
import io.github.code_akram.or2.ffi.PairOffer
import io.github.code_akram.or2.ffi.PairParseException
import io.github.code_akram.or2.ffi.PairResult
import io.github.code_akram.or2.ffi.PublicKeyInfo
import io.github.code_akram.or2.ffi.pairEnroll
import io.github.code_akram.or2.ffi.pairNewCode
import io.github.code_akram.or2.ffi.parsePairPayload
import io.github.code_akram.or2.hosts.AddressDraft
import io.github.code_akram.or2.hosts.validHost
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

/** The native code and parser as the flow uses them: [NativePair] in the app, fakes in tests. */
interface PairBackend {
    /** A new pairing code `K` from the operating system's random source. */
    fun newCode(): PairCode

    /** @throws PairParseException when [text] is not a valid pairing code. */
    fun parse(text: String): PairOffer

    /**
     * Enrols [publicKeyLine] with the host that made [offer], logging in with the key derived from [code], and returns
     * once the host installed it. @throws PairException
     */
    suspend fun enroll(offer: PairOffer, code: PairCode, publicKeyLine: String, deviceLabel: String): PairResult
}

object NativePair : PairBackend {
    override fun newCode() = pairNewCode()
    override fun parse(text: String) = parsePairPayload(text)
    override suspend fun enroll(offer: PairOffer, code: PairCode, publicKeyLine: String, deviceLabel: String) =
        pairEnroll(offer, code, publicKeyLine, deviceLabel)
}

/** Saves a paired host together with its trusted host key, in one step. */
interface PairStore {
    /**
     * Stores [host] (id 0 is new) with [hostKey] as its trusted host key, atomically, and returns the stored
     * host. Called before the first connection, so that connection finds the key already trusted.
     */
    suspend fun saveTrustedHost(host: Host, hostKey: PublicKeyInfo): Host
}

class DaoPairStore(private val dao: AppDao) : PairStore {
    override suspend fun saveTrustedHost(host: Host, hostKey: PublicKeyInfo): Host {
        val id = dao.saveHostWithTrust(host, hostKey)
        return checkNotNull(dao.host(id)) { "The paired host was not stored." }
    }
}

/** Which of the phone's keys is authorized on the host. */
sealed interface KeyChoice {
    data class Existing(val keyId: String) : KeyChoice
    data object New : KeyChoice
}

/** The first stored key, or a new one when there is none. */
fun defaultKeyChoice(keys: List<KeyRecord>): KeyChoice = keys.firstOrNull()?.let { KeyChoice.Existing(it.id) } ?: KeyChoice.New

/**
 * The pairing code `K` as the screen shows it (`7KQ4-M2XD-9PTM`). It is on the screen on purpose, so it is
 * the one piece of state that must never reach a log: [toString] hides it.
 */
data class ShownCode(val text: String) {
    override fun toString() = "ShownCode(<redacted>)"

    companion object {
        /** Before any code was drawn (a screen with no flow behind it). */
        val None = ShownCode("")
    }
}

/**
 * Whether a failed enrolment touched the host, so that the code is spent and the screen draws a new one: the
 * login with it was at least attempted. Not so when nothing could connect, the host presented another key (the
 * connection ends before anything is sent), or the phone refused its own input.
 */
fun reachedHost(error: PairException): Boolean = when (error) {
    is PairException.NoPairingId, is PairException.InvalidOffer, is PairException.InvalidKey,
    is PairException.InvalidDevice, is PairException.Unreachable, is PairException.HostKeyMismatch -> false
    else -> true
}

/** What the review screen edits: the offer and the choices made about it. */
data class PairReview(
    val offer: PairOffer,
    val name: String,
    val username: String,
    val choice: KeyChoice,
    /** Why the last attempt failed; shown above the button. */
    val error: String? = null,
    /** The fingerprint of the key the host already took, when only saving the host failed: no second enrolment. */
    val acceptedKey: String? = null,
    /** An attempt is running (asking for the biometric); the button is off. */
    val working: Boolean = false,
) {
    /** Whether the host will install the key itself (the code has a pairing id); without one the user does it by hand. */
    val enrolls get() = offer.pairingId != null

    /** The label, user and addresses make a host the app can save. */
    val valid get() = validHost(
        name, offer.addresses.map { AddressDraft(it.host, it.port.toString()) }, username,
    )
}

sealed interface PairState {
    /**
     * Showing [code] and waiting for the host's QR, from the camera or pasted; [error] explains the last code
     * that was refused or the last pairing that failed.
     */
    data class Scanning(val code: ShownCode, val error: String? = null) : PairState

    data class Review(val review: PairReview) : PairState

    /** The phone is logged in to the host's sshd with [PairReview.offer]'s pairing id, sending its key. */
    data class Pairing(val review: PairReview) : PairState

    /** A code without a pairing id: the host is saved and trusted, and the key waits to be installed by hand. */
    data class KeyToInstall(val host: Host, val keyLine: String, val fingerprint: String) : PairState

    /** Done: the host is saved with its key trusted. The screen connects to it and calls [PairFlow.consume]. */
    data class Paired(val host: Host) : PairState
}

/**
 * Easy pair on the phone: show a code, scan or paste the host's QR, review, enrol the key over the host's
 * sshd, save the host with its host key trusted. Logic only (the screens and the camera are elsewhere), over
 * [PairBackend] and [PairStore].
 *
 * Rules:
 * - the code `K` lives in Rust ([PairCode]); a new one is drawn each time the screen opens ([start]) and after
 *   every pairing that reached the host ([reachedHost]), and it is never logged or stored;
 * - the host is saved, with the host key from the code trusted, only after the host installed the key (or at
 *   once for a code without a pairing id), and before anything connects;
 * - a key generated for the pairing is saved first, so a failed pairing leaves a key the retry reuses.
 */
class PairFlow(
    private val backend: PairBackend,
    private val store: PairStore,
    private val scope: CoroutineScope,
    /** Words for a failure while generating or saving the phone's key (biometric cancelled, vault errors). */
    private val describeKeyError: (Throwable) -> String = { "The key could not be created. Try again." },
) {
    private var code = backend.newCode()
    private val mutableState = MutableStateFlow<PairState>(PairState.Scanning(ShownCode(code.display())))
    val state: StateFlow<PairState> = mutableState.asStateFlow()
    private var job: Job? = null

    /** Draws a new code and frees the old one (it zeroizes on drop). */
    private fun renewCode() {
        val spent = code
        code = backend.newCode()
        spent.close()
    }

    /** A new code, and the state that shows it. */
    private fun scanning(error: String? = null): PairState.Scanning {
        renewCode()
        return PairState.Scanning(ShownCode(code.display()), error)
    }

    /** The screen opens: drop whatever was in progress and show a new code. */
    fun start() {
        job?.cancel()
        mutableState.value = scanning()
    }

    /** The scanner or the paste field produced [text]. Ignored unless the flow is scanning. Returns whether it was taken. */
    fun onCode(text: String, keys: List<KeyRecord>): Boolean {
        val scanning = mutableState.value as? PairState.Scanning ?: return false
        val offer = try {
            backend.parse(text)
        } catch (error: PairParseException) {
            mutableState.value = scanning.copy(error = pairParseMessage(error))
            return false
        }
        mutableState.value = PairState.Review(
            PairReview(offer, name = offer.name, username = offer.username, choice = defaultKeyChoice(keys)),
        )
        return true
    }

    fun edit(name: String? = null, username: String? = null, choice: KeyChoice? = null) {
        val current = (mutableState.value as? PairState.Review)?.review ?: return
        if (current.working) return
        // With a pairing id the host authorizes the key for the one account that ran or2-pair, and the
        // code names it: a different login here would pair one account and connect as another.
        val username = if (current.enrolls) null else username
        mutableState.value = PairState.Review(
            current.copy(
                name = name ?: current.name, username = username ?: current.username,
                choice = choice ?: current.choice, error = null,
            ),
        )
    }

    /** Back from the review to scanning. The code stays: the host may be waiting for it, and nothing was sent. */
    fun rescan() {
        job?.cancel()
        mutableState.value = PairState.Scanning(ShownCode(code.display()))
    }

    /**
     * Enrols the chosen key with the host, saves the host with its key trusted, and ends in [PairState.Paired]
     * (or [PairState.KeyToInstall] for a code without a pairing id). A failure that did not reach the host returns to
     * the review with the reason; one that did returns to scanning with a new code and the reason.
     * [generateKey] creates and stores a new key (it asks for the biometric); it is called only for [KeyChoice.New].
     */
    fun submit(keys: List<KeyRecord>, device: String, generateKey: suspend (label: String, comment: String) -> KeyRecord) {
        val review = (mutableState.value as? PairState.Review)?.review ?: return
        if (review.working) return
        if (!review.valid) {
            mutableState.value = PairState.Review(review.copy(error = "Enter a name and a user name for this host."))
            return
        }
        mutableState.value = PairState.Review(review.copy(error = null, working = true))
        job = scope.launch { run(review, keys, device, generateKey) }
    }

    private suspend fun run(
        review: PairReview, keys: List<KeyRecord>, device: String,
        generateKey: suspend (String, String) -> KeyRecord,
    ) {
        fun fail(from: PairReview, message: String) {
            mutableState.value = PairState.Review(from.copy(error = message, working = false))
        }
        val key = when (val choice = review.choice) {
            is KeyChoice.Existing -> keys.find { it.id == choice.keyId } ?: return fail(review, "That key was deleted. Choose another.")
            KeyChoice.New -> try {
                generateKey("Key for ${review.name.trim()}", "or2@${device.trim()}")
            } catch (error: CancellationException) {
                throw error
            } catch (error: Exception) {
                return fail(review, describeKeyError(error))
            }
        }
        // A key made just now is stored: a retry after a failed pairing picks it up as an existing one.
        val kept = if (review.choice == KeyChoice.New) review.copy(choice = KeyChoice.Existing(key.id)) else review
        val enrolls = review.enrolls
        if (enrolls && kept.acceptedKey != key.fingerprint) {
            mutableState.value = PairState.Pairing(kept)
            try {
                backend.enroll(review.offer, code, key.openssh, device)
            } catch (error: PairException) {
                currentCoroutineContext().ensureActive() // Cancelled meanwhile: the screen was left, not failed.
                val message = pairErrorMessage(error, review.name.trim(), review.offer.port.toInt())
                if (!reachedHost(error)) return fail(kept, message)
                // The host saw this code: it is spent, and the host's run needs a new one.
                mutableState.value = scanning(message)
                return
            }
            // Success spends the code too, whatever happens when saving: the next screen draws a new one.
            renewCode()
        }
        val saved = try {
            store.saveTrustedHost(
                Host(
                    HostRecord(0, review.name.trim(), review.username.trim(), key.id),
                    review.offer.addresses.map { HostEndpoint(it.host, it.port.toInt()) },
                ),
                review.offer.hostKey,
            )
        } catch (error: CancellationException) {
            throw error
        } catch (error: Exception) {
            // The host has the key already: a retry saves the host without pairing again.
            return fail(
                kept.copy(acceptedKey = key.fingerprint.takeIf { enrolls }),
                "The host accepted the key, but this phone could not save the host. Try again.",
            )
        }
        mutableState.value =
            if (enrolls) PairState.Paired(saved) else PairState.KeyToInstall(saved, key.openssh, key.fingerprint)
    }

    /** Takes the paired host (or ends the key-to-install screen) and returns to scanning. */
    fun consume(): Host? {
        val host = when (val current = mutableState.value) {
            is PairState.Paired -> current.host
            is PairState.KeyToInstall -> current.host
            else -> null
        }
        if (host != null) mutableState.value = scanning()
        return host
    }

    /** The user left the pairing screens: abort whatever runs and forget everything, including the code. */
    fun cancel() {
        job?.cancel()
        job = null
        mutableState.value = scanning()
    }
}
