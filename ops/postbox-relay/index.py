"""Private Yandex Cloud Function; no dependencies, SMTP, or raw event logs."""
import hashlib
import hmac
import json
import os
import re
import ssl
import time
import urllib.request

URL = "https://pandaolate.ru/internal/mail/postbox-feedback"
PREFIX = b"tor/postbox/relay/v1\n"
MAX_BATCH = 5
MAX_BODY = 65536
TIMEOUT = 8
IDENTIFIER = re.compile(r"[A-Za-z0-9_.:-]{1,128}\Z")


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def identifier(value):
    if not isinstance(value, str) or not IDENTIFIER.fullmatch(value):
        raise ValueError("invalid identifier")
    return value


def select_event(event, expected_identity):
    """Reduce to the exact fields accepted by TOR; never send full headers."""
    if not isinstance(event, dict):
        raise ValueError("invalid event")
    kind = event.get("eventType")
    if kind not in ("Bounce", "Complaint"):
        return None
    mail = event["mail"]
    if mail["identityId"] != expected_identity:
        raise ValueError("unexpected identity")
    if kind == "Bounce":
        data = event["bounce"]
        if (data.get("bounceType"), data.get("bounceSubType")) != (
            "Permanent", "Undetermined"
        ):
            return None
        recipients = data["bouncedRecipients"]
        if not isinstance(recipients, list) or len(recipients) != 1:
            raise ValueError("ambiguous recipient")
        recipient = recipients[0]
        if recipient.get("status") != "5.1.1" or recipient.get("action", "failed") != "failed":
            return None
        detail = {
            "bounce": {
                "bounceType": "Permanent",
                "bounceSubType": "Undetermined",
                "bouncedRecipients": [{
                    "emailAddress": recipient["emailAddress"],
                    "status": "5.1.1",
                    "action": "failed",
                }],
            }
        }
    else:
        data = event["complaint"]
        if data.get("complaintFeedbackType") != "abuse":
            return None
        recipients = data["complainedRecipients"]
        if not isinstance(recipients, list) or len(recipients) != 1:
            raise ValueError("ambiguous recipient")
        recipient = recipients[0]
        detail = {
            "complaint": {
                "complaintFeedbackType": "abuse",
                "complainedRecipients": [{"emailAddress": recipient["emailAddress"]}],
            }
        }
    address = recipient["emailAddress"]
    if (not isinstance(address, str) or not address.isascii()
            or not 3 <= len(address) <= 254
            or address.count("@") != 1
            or any(ord(c) <= 32 or ord(c) == 127 for c in address)):
        raise ValueError("invalid recipient")
    result = {
        "eventType": kind,
        "eventId": identifier(event["eventId"]),
        "mail": {
            "identityId": identifier(mail["identityId"]),
            "messageId": identifier(mail["messageId"]),
        },
        **detail,
    }
    marker = mail.get("commonHeaders", {}).get("originalMessageId")
    if marker not in (None, ""):
        if not isinstance(marker, str) or len(marker) > 128:
            raise ValueError("invalid marker")
        result["mail"]["commonHeaders"] = {"originalMessageId": marker}
    return result


def load_key(value):
    if not re.fullmatch(r"[0-9a-fA-F]{64}", value):
        raise ValueError("invalid relay key")
    key = bytes.fromhex(value)
    if not any(key):
        raise ValueError("invalid relay key")
    return key


def sign(key, timestamp, body):
    return hmac.new(key, PREFIX + timestamp.encode("ascii") + b"\n" + body,
                    hashlib.sha256).hexdigest()


def submit(opener, key, event):
    body = json.dumps(event, ensure_ascii=True, separators=(",", ":")).encode("ascii")
    if len(body) > MAX_BODY:
        raise ValueError("oversized relay body")
    timestamp = str(int(time.time()))
    request = urllib.request.Request(
        URL, data=body, method="POST",
        headers={
            "Content-Type": "application/json",
            "X-Tor-Mail-Timestamp": timestamp,
            "X-Tor-Mail-Signature": sign(key, timestamp, body),
        },
    )
    with opener.open(request, timeout=TIMEOUT) as response:
        if response.status != 204:
            raise ValueError("relay not acknowledged")


def handle_batch(event, key, identity, opener):
    messages = event["messages"]
    if not isinstance(messages, list) or not 1 <= len(messages) <= MAX_BATCH:
        raise ValueError("invalid batch")
    accepted = 0
    for raw in messages:
        selected = select_event(raw, identity)
        if selected is not None:
            submit(opener, key, selected)
            accepted += 1
    return {"accepted": accepted}


def handler(event, context):
    # Invoke only from the dedicated, IAM-restricted Data Streams trigger.
    # Do not print exceptions, requests, cloud event payloads or secrets.
    try:
        key = load_key(os.environ["TOR_POSTBOX_RELAY_KEY"])
        identity = identifier(os.environ["TOR_POSTBOX_IDENTITY_ID"])
        opener = urllib.request.build_opener(
            urllib.request.ProxyHandler({}),
            NoRedirect(),
            urllib.request.HTTPSHandler(context=ssl.create_default_context()),
        )
        return handle_batch(event, key, identity, opener)
    except Exception:
        # A failed invocation lets the trigger retry the batch. A previously
        # committed event is safe to replay via its durable event-ID fence.
        raise RuntimeError("TOR_POSTBOX_RELAY_RETRY") from None
