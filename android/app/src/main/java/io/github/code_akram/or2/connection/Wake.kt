package io.github.code_akram.or2.connection

import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.HostEndpoint
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.inbox.LinkStatus
import io.github.code_akram.or2.inbox.isSleepFailure
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first
import java.net.Inet4Address
import java.net.InetAddress

/** What a host card shows of a Wake: one in progress ("Waking…"), or one that gave up on a sleeping host. */
enum class WakeStatus { WAKING, CANT_WAKE }

/**
 * A Wake that ran out of time on a host marked "sleeps". The app cannot get round it: a MacBook with its lid closed on
 * battery sleeps and then enters standby with Wi-Fi off, so neither the packet nor the probe reaches it.
 */
const val CANT_WAKE_MESSAGE = "Can't wake: it may be asleep with the lid closed or on battery"

/** How long Wake keeps trying: no attempt starts after this, counted from the tap. */
const val WAKE_BUDGET_MS = 30_000L

/** The pause between a connect that found no one and the next try. */
const val WAKE_RETRY_MS = 2_000L

/** What one connect attempt of a Wake came to. */
enum class WakeAttempt {
    /** The host answered (it connected, or asks for a host-key decision): Wake is done. */
    ANSWERED,

    /** Nobody answered (unreachable, timed out, lost): still asleep, try again. */
    NO_ANSWER,

    /** The host answered and refused (a rejected key, a bad host key), or the user ended it: retrying cannot help. */
    FAILED,
}

/**
 * Whether the host menu offers Wake for [host] whose connection reads [link]: it is asleep or not connected, and it
 * has a way to be woken (a MAC address for the packet, or the probe).
 */
fun canWake(host: Host, link: LinkStatus): Boolean = link.canConnect && (host.macAddress != null || host.wakeProbe)

/** A connection's settled [state] as a Wake attempt's outcome. */
fun wakeAttemptOf(state: HostState): WakeAttempt = when (state) {
    is HostState.Connected, is HostState.AwaitingHostKeyDecision -> WakeAttempt.ANSWERED
    is HostState.Closed -> when (val reason = state.reason) {
        is CloseReason.Failed -> if (isSleepFailure(reason.failure)) WakeAttempt.NO_ANSWER else WakeAttempt.FAILED
        else -> WakeAttempt.FAILED
    }
    else -> WakeAttempt.FAILED
}

/**
 * One Wake attempt over the app's connections: connects [host] with a copy of [key] (a connect wipes the array it is
 * given; the caller's copy serves the next attempt and is wiped by its unlock) and waits until the connection settles.
 */
suspend fun HostConnections.wakeAttempt(host: Host, key: ByteArray): WakeAttempt {
    connect(host, key.copyOf())
    val active = this.host(host.id) ?: return WakeAttempt.FAILED
    return wakeAttemptOf(active.state.first { it != HostState.Connecting && it != HostState.Authenticating })
}

/** The host's connection as a Wake drives it: its key, unlocked once for every attempt. */
fun interface WakeConnection {
    /**
     * Unlocks the host's key (the biometric prompt) and runs [attempts] with a function that makes one connect
     * attempt with it. The key is wiped once [attempts] returns or throws.
     */
    suspend fun unlocked(attempts: suspend (connectOnce: suspend () -> WakeAttempt) -> Unit)
}

/**
 * Home's Wake ([wake]): the magic packet ([sendPacket], for a host with a MAC address, to the current network's
 * [broadcasts]) and the TCP wake probe ([probe], for a host with it on), then connects, trying again for up to
 * [budgetMs] while nobody answers. [status] is what each host card shows meanwhile ("Waking…"), and
 * [WakeStatus.CANT_WAKE] for a host marked "sleeps" that never answered ([CANT_WAKE_MESSAGE]); a host that does not
 * sleep shows its connection's own failure instead.
 *
 * The first packet and probe go out before the unlock, so the host is waking while the user authenticates. A retry
 * sends the packet again (a broadcast is not acknowledged and Wi-Fi drops one now and then); the probe it does not
 * repeat, because every connect of a host with the probe on runs it first ([HostConnections]).
 */
class Waker(
    private val sendPacket: suspend (mac: String, broadcasts: List<String>) -> Unit,
    private val probe: suspend (List<HostEndpoint>) -> Unit,
    private val broadcasts: () -> List<String>,
    private val monotonicMs: () -> Long = { System.nanoTime() / 1_000_000 },
    private val budgetMs: Long = WAKE_BUDGET_MS,
    private val retryMs: Long = WAKE_RETRY_MS,
) {
    private val mutableStatus = MutableStateFlow<Map<Long, WakeStatus>>(emptyMap())

    /** Hosts being woken or given up on, by id. */
    val status: StateFlow<Map<Long, WakeStatus>> = mutableStatus.asStateFlow()

    /** Forgets a host's given-up Wake: a new connect or Wake of it speaks for itself. */
    fun clear(hostId: Long) {
        if (mutableStatus.value[hostId] == WakeStatus.CANT_WAKE) mutableStatus.value -= hostId
    }

    /**
     * Wakes [host] and connects it through [connection]; returns once it answered, refused, or the time is up. An
     * error from the connection (a cancelled unlock, an invalid request) ends the Wake and is rethrown.
     */
    suspend fun wake(host: Host, connection: WakeConnection) {
        val started = monotonicMs()
        mutableStatus.value += host.id to WakeStatus.WAKING
        var gaveUp = false
        try {
            knock(host)
            if (host.wakeProbe) probe(host.addresses)
            connection.unlocked { connectOnce ->
                while (connectOnce() == WakeAttempt.NO_ANSWER) {
                    val left = budgetMs - (monotonicMs() - started)
                    if (left <= 0) {
                        gaveUp = true
                        return@unlocked
                    }
                    delay(minOf(retryMs, left))
                    knock(host)
                }
            }
        } finally {
            mutableStatus.value = if (gaveUp && host.sleeps) mutableStatus.value + (host.id to WakeStatus.CANT_WAKE)
            else mutableStatus.value - host.id
        }
    }

    /** The magic packet, for a host with a MAC address. Failing to send it (no network) is not the end: the connect says why. */
    private suspend fun knock(host: Host) {
        val mac = host.macAddress ?: return
        try {
            sendPacket(mac, broadcasts())
        } catch (error: CancellationException) {
            throw error
        } catch (_: Exception) {
            // The connect that follows explains an unreachable network in its own words.
        }
    }
}

/**
 * The subnet-directed broadcast address of an IPv4 link address with [prefixLength] (`192.168.1.20/24` gives
 * `192.168.1.255`); null for an IPv6 address, and for a prefix that leaves no broadcast address (/31 and /32, which
 * mobile data often hands out, and /0).
 */
fun directedBroadcast(address: InetAddress, prefixLength: Int): String? {
    if (address !is Inet4Address || prefixLength !in 1..30) return null
    val bits = address.address.fold(0L) { acc, byte -> (acc shl 8) or (byte.toLong() and 0xff) }
    val host = (1L shl (32 - prefixLength)) - 1
    val broadcast = bits or host
    return (3 downTo 0).joinToString(".") { ((broadcast shr (8 * it)) and 0xff).toString() }
}

/** [directedBroadcast] of each link address, each once, in order. */
fun directedBroadcasts(links: List<Pair<InetAddress, Int>>): List<String> =
    links.mapNotNull { (address, prefix) -> directedBroadcast(address, prefix) }.distinct()
