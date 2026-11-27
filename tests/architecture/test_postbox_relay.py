"""MAIL-01g2b: local Python relay tests; no cloud or network traffic."""
import importlib.util
import json
from pathlib import Path
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "postbox_relay", ROOT / "ops/postbox-relay/index.py"
)
relay = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(relay)


def bounce():
    return {
        "eventType": "Bounce",
        "eventId": "event-1:0",
        "mail": {
            "identityId": "identity-fixture",
            "messageId": "message-1",
            "commonHeaders": {"originalMessageId": "<tor-" + "ab" * 32 + "@pandaolate.ru>",
                              "subject": "PRIVATE SUBJECT"},
            "headers": [{"name": "Private", "value": "PRIVATE HEADER"}],
            "tags": {"private": "PRIVATE TAG"},
        },
        "bounce": {
            "bounceType": "Permanent",
            "bounceSubType": "Undetermined",
            "bouncedRecipients": [{"emailAddress": "reader@example.invalid",
                                   "status": "5.1.1", "action": "failed",
                                   "diagnosticCode": "PRIVATE DIAGNOSTIC"}],
        },
    }


class Response:
    def __init__(self, status):
        self.status = status

    def __enter__(self):
        return self

    def __exit__(self, *args):
        pass


class Opener:
    def __init__(self, status=204):
        self.status = status
        self.requests = []

    def open(self, request, timeout):
        self.requests.append((request, timeout))
        return Response(self.status)


class PostboxRelayTests(unittest.TestCase):
    def test_cross_language_signature_vector(self):
        self.assertEqual(
            relay.sign(bytes.fromhex("a5" * 32), "1700000000", b'{"eventType":"Delivery"}'),
            "6c2eb55dcb191904155f2beac2f4629402dc8d7115fad6598a229d1d8076751f",
        )

    def test_minimization_and_signed_post(self):
        opener = Opener()
        key = relay.load_key("a5" * 32)
        with patch.object(relay.time, "time", return_value=1700000000):
            self.assertEqual(
                relay.handle_batch({"messages": [bounce(), {"eventType": "Delivery"}]},
                                   key, "identity-fixture", opener),
                {"accepted": 1},
            )
        self.assertEqual(len(opener.requests), 1)
        req, timeout = opener.requests[0]
        self.assertEqual(req.full_url, relay.URL)
        self.assertEqual(req.get_method(), "POST")
        self.assertEqual(timeout, 8)
        self.assertNotIn(b"PRIVATE", req.data)
        body = json.loads(req.data)
        self.assertEqual(body["eventId"], "event-1:0")
        self.assertEqual(body["bounce"]["bouncedRecipients"][0]["status"], "5.1.1")
        self.assertEqual(req.get_header("X-tor-mail-timestamp"), "1700000000")
        self.assertEqual(req.get_header("X-tor-mail-signature"),
                         relay.sign(key, "1700000000", req.data))

    def test_unrelated_failures_and_non_abuse_complaints_not_sent(self):
        for status in ("5.7.1", "5.2.2", "4.4.1"):
            event = bounce()
            event["bounce"]["bouncedRecipients"][0]["status"] = status
            self.assertIsNone(relay.select_event(event, "identity-fixture"))
        event = bounce()
        event["bounce"]["bounceSubType"] = "Suppressed"
        self.assertIsNone(relay.select_event(event, "identity-fixture"))
        event = bounce()
        event["eventType"] = "Complaint"
        event["complaint"] = {"complaintFeedbackType": "not-spam"}
        self.assertIsNone(relay.select_event(event, "identity-fixture"))
        event["complaint"] = {
            "complaintFeedbackType": "abuse",
            "complainedRecipients": [{"emailAddress": "reader@example.invalid"}],
        }
        self.assertIn("complaint", relay.select_event(event, "identity-fixture"))

    def test_foreign_identity_and_bad_batches_fail(self):
        with self.assertRaises(ValueError):
            relay.select_event(bounce(), "foreign")
        for messages in ([], [bounce()] * 6, "not an array"):
            with self.assertRaises(ValueError):
                relay.handle_batch({"messages": messages}, b"x" * 32,
                                   "identity-fixture", Opener())
        event = bounce()
        event["bounce"]["bouncedRecipients"] *= 2
        with self.assertRaises(ValueError):
            relay.select_event(event, "identity-fixture")
        for key in ("", "00" * 32, "x" * 64):
            with self.assertRaises(ValueError):
                relay.load_key(key)

    def test_ack_only_204_no_redirect_and_safe_batch_retry(self):
        for status in (200, 301, 401, 403, 409, 429, 500, 503):
            with self.subTest(status=status), self.assertRaises(ValueError):
                relay.handle_batch({"messages": [bounce()]}, b"x" * 32,
                                   "identity-fixture", Opener(status))
        self.assertIsNone(relay.NoRedirect().redirect_request(
            None, None, 302, "redirect", {}, "https://example.invalid"))
        with patch.dict(relay.os.environ, {"TOR_POSTBOX_RELAY_KEY": "PRIVATE_INVALID_KEY",
                                         "TOR_POSTBOX_IDENTITY_ID": "identity-fixture"}):
            with self.assertRaisesRegex(RuntimeError, "^TOR_POSTBOX_RELAY_RETRY$"):
                relay.handler({"messages": [bounce()]}, None)


if __name__ == "__main__":
    unittest.main()
