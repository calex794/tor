#!/usr/bin/env python3
"""D-04/S034: real tor-web HTTP with disposable PostgreSQL, no payment provider."""
from __future__ import annotations

import argparse
import http.cookiejar
import json
import os
import re
import subprocess
import urllib.error
import urllib.parse
import urllib.request

BASE = "http://127.0.0.1:3098"
PATH = "/api/v1/purchases"
DB = os.environ.get("TEST_DATABASE_URL", "")


def require(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def pg(sql: str) -> str:
    url = urllib.parse.urlsplit(DB)
    require(
        url.hostname in ("localhost", "127.0.0.1")
        and url.path == "/tor_ci"
        and url.username == "tor_app",
        "Only disposable local tor_ci may be used for money tests",
    )
    result = subprocess.run(
        ["psql", "-X", "-A", "-t", "-q", "-v", "ON_ERROR_STOP=1",
         "--dbname", DB, "--command", sql],
        check=False, text=True, capture_output=True, timeout=20,
    )
    require(result.returncode == 0, "Disposable PG failed: " + result.stderr[-1200:])
    return result.stdout.strip()


def scalar(sql: str) -> int:
    value = pg(sql)
    require(value.isdecimal(), "PostgreSQL must return one positive integer")
    return int(value)


class Client:
    def __init__(self) -> None:
        self.cookies = http.cookiejar.CookieJar()
        self.opener = urllib.request.build_opener(
            urllib.request.HTTPCookieProcessor(self.cookies))

    def cookie(self, name: str) -> str:
        matches = [c.value for c in self.cookies if c.name == name]
        require(len(matches) == 1, "Missing or duplicate cookie: " + name)
        return matches[0]

    def call(
        self, method: str, path: str, payload: object = None,
        origin: str | None = None, csrf: str | None = None,
        key: str | None = None,
    ) -> tuple[int, dict[str, str], str]:
        headers = {"Accept": "application/json, text/html"}
        data = None
        if isinstance(payload, dict):
            data = json.dumps(payload, ensure_ascii=False).encode("utf-8")
            headers["Content-Type"] = "application/json"
        elif isinstance(payload, bytes):
            data = payload
            headers["Content-Type"] = "application/x-www-form-urlencoded"
        if origin is not None:
            headers["Origin"] = origin
        if csrf is not None:
            headers["X-CSRF-Token"] = csrf
        if key is not None:
            headers["Idempotency-Key"] = key
        req = urllib.request.Request(BASE + path, data=data, headers=headers, method=method)
        try:
            res = self.opener.open(req, timeout=15)
        except urllib.error.HTTPError as error:
            res = error
        with res:
            return res.status, {k.lower(): v for k, v in res.headers.items()}, (
                res.read(65536).decode("utf-8", errors="replace"))

    def register(self, email: str, display: str) -> None:
        status, _, page = self.call("GET", "/register")
        require(status == 200, "Registration form is missing")
        match = re.search(r'name="csrf_token"\s+value="([^"]+)"', page)
        require(match is not None, "Registration CSRF field missing")
        token = match.group(1)
        require(token == self.cookie("tor-dev-csrf"), "CSRF cookie differs from form")
        form = urllib.parse.urlencode({
            "email": email, "display_name": display,
            "password": "D04VeryStrongBuyerPassword2026", "csrf_token": token,
        }).encode("utf-8")
        status, _, _ = self.call("POST", "/register", form, origin=BASE)
        require(status == 200, "Registration failed")
        require(len(self.cookie("tor-dev-session")) == 43, "Session cookie missing")
        require(self.call("GET", "/me")[0] == 200, "User profile not accessible")


def expect(reply: tuple[int, dict[str, str], str], status: int, label: str) -> None:
    require(reply[0] == status, f"{label}: expected HTTP {status}, got {reply[0]}")


def buy(client: Client, payload: dict, *, csrf: str | None = None,
        key: str | None = None, origin: str | None = BASE):
    return client.call("POST", PATH, payload, origin=origin, csrf=csrf, key=key)


def disabled() -> None:
    expect(buy(Client(), {"chapter_id": "1", "expected_version_id": "1",
                          "expected_price_minor": 150}, key="A" * 42 + "8"),
           404, "Checkout must be disabled by default")
    expect(Client().call("POST", "/api/v1/reader/chapters/1/delivery-grants", {}),
           404, "Paid delivery must also be disabled by default")
    expect(
        Client().call(
            "POST", "/api/v1/layers/1/purchase",
            {"expected_offer_id": "1", "expected_version_id": "1",
             "expected_price_minor": 90},
            origin=BASE, key="A" * 42 + "8",
        ),
        404, "Layer purchase must also be disabled by default",
    )
    print("D-04 default-off: PASS")


def enabled() -> None:
    require(DB, "Set TEST_DATABASE_URL to isolated tor_ci")
    expect(
        Client().call(
            "POST", "/api/v1/layers/1/purchase",
            {"expected_offer_id": "1", "expected_version_id": "1",
             "expected_price_minor": 90},
            origin=BASE, key="A" * 42 + "8",
        ),
        401, "Enabled Layer purchase must still require an authenticated session",
    )
    expect(Client().call("POST", "/api/v1/reader/chapters/1/delivery-grants", {}),
           404, "Checkout opt-in must not automatically enable paid S3 grants")
    buyer, author, stranger = Client(), Client(), Client()
    buyer.register("d04-smoke-buyer@tor-ci.example", "Покупатель")
    author.register("d04-smoke-author@tor-ci.example", "Автор")
    stranger.register("d04-smoke-stranger@tor-ci.example", "Посторонний")
    buyer_id = scalar("SELECT id FROM users WHERE email_normalized='d04-smoke-buyer@tor-ci.example'")
    author_id = scalar("SELECT id FROM users WHERE email_normalized='d04-smoke-author@tor-ci.example'")
    require(buyer_id != author_id, "Accounts must not share a wallet")
    pg("INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency) "
       f"VALUES ({buyer_id},600,'RUB'),({author_id},600,'RUB')")
    book = scalar("INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id) "
                  f"VALUES ('ci:d04:http-smoke','ru','Книга с покупкой','public',{author_id}) RETURNING id")
    chapter = scalar("INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount) "
                     f"VALUES ({book},1,'Платная глава','public','paid',175) RETURNING id")
    version = scalar("INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) "
                     f"VALUES ({chapter},1,'SECRET-PAID-D04-NO-LEAK') RETURNING id")
    pg(f"INSERT INTO chapter_publications(chapter_id,version_id) VALUES ({chapter},{version})")
    payload = {"chapter_id": str(chapter), "expected_version_id": str(version),
               "expected_price_minor": 175}
    key = "A" * 42 + "8"
    csrf = buyer.cookie("tor-dev-csrf")
    author_csrf = author.cookie("tor-dev-csrf")

    expect(buy(Client(), payload, csrf=csrf, key=key), 401, "Guest may not buy")
    expect(buy(buyer, payload, csrf=csrf, key=key, origin="https://evil.example"), 403,
           "Cross-origin request")
    expect(buy(buyer, payload, key=key), 403, "Missing CSRF")
    expect(buy(buyer, payload, csrf="wrong", key=key), 403, "Wrong CSRF")
    expect(buy(buyer, payload, csrf=csrf), 422, "Missing idempotency key")
    expect(buy(buyer, payload, csrf=csrf, key="bad"), 422, "Invalid key")
    expect(buy(buyer, {**payload, "user_id": author_id}, csrf=csrf, key=key), 422,
           "Client-provided actor must be rejected")
    expect(buy(author, payload, csrf=author_csrf, key=key), 404,
           "Author cannot buy their own chapter")
    require(scalar("SELECT count(*) FROM chapter_purchase_facts "
                   f"WHERE user_id={buyer_id}") == 0, "Denials caused a debit")
    expect(buyer.call("GET", f"/reader/{chapter}"), 404, "Free reader must hide paid bytes")

    status, headers, body = buy(buyer, payload, csrf=csrf, key=key)
    require(status == 200 and headers.get("cache-control") == "no-store", "First purchase failed")
    require("SECRET-PAID" not in body, "Receipt leaked paid text")
    receipt = json.loads(body)
    require(receipt["balance_minor"] == 425 and receipt["replayed"] is False,
            "Initial purchase receipt incorrect")
    require(receipt["chapter_id"] == str(chapter)
            and receipt["version_id"] == str(version), "Purchased version was not pinned")
    purchase_id = int(receipt["purchase_id"])
    status, _, data = buy(buyer, payload, csrf=csrf, key=key)
    require(status == 200 and json.loads(data)["replayed"] is True
            and json.loads(data)["purchase_id"] == str(purchase_id), "Retry not idempotent")
    expect(buy(buyer, {**payload, "expected_price_minor": 176}, csrf=csrf, key=key),
           409, "Changed price on same key")
    expect(buy(buyer, payload, csrf=csrf, key="B" * 42 + "8"),
           409, "Second key cannot buy again")
    expect(buy(stranger, payload, csrf=stranger.cookie("tor-dev-csrf"), key=key),
           409, "Unfunded stranger cannot buy")
    require(scalar("SELECT count(*) FROM chapter_purchase_facts "
                   f"WHERE user_id={buyer_id}") == 1, "Multiple purchase facts")
    require(scalar("SELECT count(*) FROM reader_wallet_entries "
                   f"WHERE user_id={buyer_id} AND kind='chapter_purchase'") == 1,
            "Duplicate ledger entry")
    require(scalar("SELECT balance_minor FROM reader_wallet_accounts "
                   f"WHERE user_id={buyer_id}") == 425, "Wrong wallet balance")
    require(scalar("SELECT count(*) FROM library_purchased_chapters "
                   f"WHERE user_id={buyer_id} AND chapter_id={chapter}") == 1,
            "Purchased pinned entitlement missing")

    uri = ("/api/v1/me/progress/purchased?"
           f"purchase_id={purchase_id}&chapter_id={chapter}&version_id={version}")
    expect(Client().call("GET", uri), 401, "Guest purchased progress")
    expect(stranger.call("GET", uri), 404, "Stranger purchased progress")
    status, _, text = buyer.call("GET", uri)
    require(status == 200 and json.loads(text)["revision"] == 0,
            "Buyer cannot query purchased position")
    require("SECRET-PAID" not in text, "Purchased progress leaked text")
    position = {"purchase_id": str(purchase_id), "chapter_id": str(chapter),
                "version_id": str(version), "block_id": 1,
                "char_offset": 0, "expected_revision": 0}
    status, _, text = buyer.call("PUT", "/api/v1/me/progress/purchased", position,
                                 origin=BASE, csrf=csrf)
    require(status == 200 and json.loads(text)["revision"] == 1,
            "Reader failed to save paid position")
    expect(stranger.call("PUT", "/api/v1/me/progress/purchased", position,
                         origin=BASE, csrf=stranger.cookie("tor-dev-csrf")),
           404, "Stranger may not save paid progress")
    v2 = scalar("INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) "
                f"VALUES ({chapter},2,'Новая редакция') RETURNING id")
    pg(f"UPDATE chapter_publications SET version_id={v2} WHERE chapter_id={chapter}")
    status, _, text = buyer.call("GET", uri)
    require(status == 200 and json.loads(text)["revision"] == 1
            and json.loads(text)["version_id"] == str(version),
            "V2 overwrote the purchased V1 position")
    pg(f"UPDATE books SET visibility='draft' WHERE id={book}")
    expect(buyer.call("GET", uri), 200, "Author hiding storefront revoked purchased V1")
    pg(f"UPDATE books SET admin_hold=true WHERE id={book}")
    expect(buyer.call("GET", uri), 404, "Moderation must deny access")
    pg(f"UPDATE books SET admin_hold=false WHERE id={book}")
    pg("INSERT INTO chapter_purchase_revocations(purchase_id,reversal_reference,reason) "
       f"VALUES ({purchase_id},'d04-http-smoke-reversal','refund')")
    expect(buyer.call("GET", uri), 404, "Refund must revoke progress access")
    expect(buyer.call("PUT", "/api/v1/me/progress/purchased", position,
                      origin=BASE, csrf=csrf), 404, "Refund must reject position writes")
    print("D-04/S034 real HTTP+PostgreSQL: PASS (sessions/CSRF/replay/ledger/V1/refund)")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--mode", choices=("disabled", "enabled"), required=True)
    args = parser.parse_args()
    if args.mode == "disabled":
        disabled()
    else:
        enabled()


if __name__ == "__main__":
    main()
