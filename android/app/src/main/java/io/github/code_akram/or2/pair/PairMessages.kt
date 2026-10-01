package io.github.code_akram.or2.pair

import io.github.code_akram.or2.ffi.PairException
import io.github.code_akram.or2.ffi.PairParseException

/** Why a scanned or pasted code was refused, for the line under the scanner. */
fun pairParseMessage(error: PairParseException): String = when (error) {
    is PairParseException.NotPairingCode ->
        "That is not an or2 pairing code. Run or2-pair on the host and scan the code it prints."
    is PairParseException.UnsupportedVersion -> "This code is from a newer or2-pair. Update or2 on this phone."
    is PairParseException.TooLong -> "That text is too long to be a pairing code."
    is PairParseException.Malformed -> "The pairing code is damaged. Scan it again, or paste its text."
    is PairParseException.MissingField, is PairParseException.InvalidField, is PairParseException.DuplicateField,
    is PairParseException.UnknownField -> "The pairing code is not valid. Run or2-pair again and scan the new code."
}

/** Why the key was not accepted, for the review screen. Nothing here carries the code or its password. */
fun pairErrorMessage(error: PairException): String = when (error) {
    is PairException.Declined -> "The host declined the key, so nothing was changed. Run or2-pair again to retry."
    is PairException.AuthenticationFailed ->
        "The host did not accept the code. It may already have been used: run or2-pair again and scan the new one."
    is PairException.HostTimedOut, is PairException.TimedOut ->
        "Nobody confirmed on the host in time. Run or2-pair again and answer y at its prompt."
    is PairException.Unreachable ->
        "Could not reach the host to pair. The phone must be on the same network (or share an overlay such as " +
            "ZeroTier or Tailscale). Otherwise run or2-pair with --no-listen and add the key by hand."
    is PairException.ConnectionLost -> "The connection to the host ended early. Run or2-pair again and retry."
    is PairException.Protocol, is PairException.BadRequest, is PairException.Refused ->
        "The host did not understand the request. Update or2-pair on the host and try again."
    is PairException.KeyNotAccepted -> "The host does not accept this kind of key. Pick or generate an Ed25519 key."
    is PairException.InvalidKey -> "This key cannot be sent to the host. Pick another key."
    is PairException.InvalidDevice -> "This phone's name cannot be sent to the host."
    is PairException.NoExchange -> "This code has no listener: install the key on the host by hand."
    is PairException.Wiped, is PairException.InvalidOffer -> "This pairing code was already used. Scan a new one."
}
