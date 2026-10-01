package io.github.code_akram.or2.pair

import io.github.code_akram.or2.ffi.PairException
import io.github.code_akram.or2.ffi.PairParseException
import io.github.code_akram.or2.ffi.parsePairPayload
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

/** Every refusal has words a person can act on, and none of them carries the code or its password. */
class PairMessagesTest {
    private val everyPairError = listOf(
        PairException.NoExchange(), PairException.Wiped(), PairException.InvalidOffer(), PairException.InvalidKey(),
        PairException.InvalidDevice(), PairException.Unreachable(), PairException.TimedOut(), PairException.Protocol(),
        PairException.ConnectionLost(), PairException.Declined(), PairException.AuthenticationFailed(),
        PairException.KeyNotAccepted(), PairException.HostTimedOut(), PairException.BadRequest(), PairException.Refused(),
    )

    private val everyParseError = listOf(
        PairParseException.NotPairingCode(), PairParseException.UnsupportedVersion(), PairParseException.TooLong(),
        PairParseException.Malformed(), PairParseException.MissingField("user"), PairParseException.DuplicateField("a"),
        PairParseException.UnknownField(), PairParseException.InvalidField("otp"),
    )

    @Test
    fun everyPairErrorHasASentenceThatNamesWhatToDo() {
        for (error in everyPairError) {
            val message = pairErrorMessage(error)
            assertTrue("${error::class.simpleName}: $message", message.length > 20 && message.endsWith("."))
        }
    }

    @Test
    fun everyParseErrorHasASentence() {
        for (error in everyParseError) {
            val message = pairParseMessage(error)
            assertTrue("${error::class.simpleName}: $message", message.length > 20 && message.endsWith("."))
        }
    }

    @Test
    fun theRealParserRaisesTheErrorsTheMessagesAreWrittenFor() {
        for ((text, expected) in listOf(
            "not a code" to PairParseException.NotPairingCode::class,
            "or2-pair:9?x=y" to PairParseException.UnsupportedVersion::class,
            "or2-pair:1?" to PairParseException.Malformed::class,
            "or2-pair:1?name=x" to PairParseException.MissingField::class,
        )) {
            try {
                parsePairPayload(text)
                fail("$text should not parse")
            } catch (error: PairParseException) {
                assertEquals(text, expected, error::class)
            }
        }
    }

    @Test
    fun noMessageCarriesAPasswordOrACode() {
        val all = everyPairError.map(::pairErrorMessage) + everyParseError.map(::pairParseMessage)
        for (message in all) {
            assertFalse(message, message.contains("otp"))
            assertFalse(message, message.contains("or2-pair:1?"))
        }
    }
}
