package io.github.code_akram.or2.connection

import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalTransport
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.map

/**
 * For the latest collection [this] emits (the hosts, the terminals, a host's watches), the latest value of each
 * element's own flow ([each]), in order; an empty collection is an empty list at once.
 */
@OptIn(ExperimentalCoroutinesApi::class)
inline fun <T, reified R> Flow<Collection<T>>.combineEach(noinline each: (T) -> Flow<R>): Flow<List<R>> = flatMapLatest { items ->
    if (items.isEmpty()) flowOf(emptyList()) else combine(items.map(each)) { it.toList() }
}

/** The real transport of every open terminal, keyed by terminal id (it changes when AUTO falls back). */
fun HostConnections.transports(): Flow<Map<Long, TerminalTransport>> =
    terminals.combineEach { t -> t.transport.map { t.id to it } }.map { it.toMap() }

/** Whether each open terminal has closed, keyed by terminal id. */
fun HostConnections.terminalClosedStates(): Flow<Map<Long, Boolean>> =
    terminals.combineEach { t -> t.state.map { t.id to (it is SessionState.Closed) } }.map { it.toMap() }
