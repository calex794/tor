#!/usr/bin/env python3
"""PERF-252 only: disposable wallets and paid book for real HTTP purchases."""
import os
import sys
from pathlib import Path
from urllib.parse import urlsplit

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools"))
import checkout_http_pg_smoke as smoke

db = urlsplit(os.environ.get("TEST_DATABASE_URL", ""))
assert (os.environ.get("GITHUB_EVENT_NAME") == "pull_request"
        and os.environ.get("PGHOST") == "127.0.0.1"
        and db.hostname == "127.0.0.1" and db.path == "/tor_ci"
        and db.username == "tor_app"), "refusing non-disposable database"
smoke.BASE = "http://127.0.0.1:3089"
author = smoke.Client()
author.register("perf252-paid-author@tor-ci.example", "PERF252 author")
buyers = 8
for n in range(1, buyers + 1):
    client = smoke.Client()
    client.register(f"perf252-paid-buyer-{n}@tor-ci.example", f"PERF252 buyer {n}")
owner = smoke.scalar(
    "SELECT id FROM users WHERE email_normalized='perf252-paid-author@tor-ci.example'")
smoke.require(owner > 0, "author registration failed")
smoke.pg(f"""INSERT INTO books(id,source_identity,target_language,title,visibility,owner_user_id)
VALUES(20001,'ci:perf252:paid-book','ru','PERF252 покупка','public',{owner})""")
smoke.pg("""INSERT INTO chapters(id,book_id,chapter_no,title,visibility,access_level,price_amount)
SELECT 2000000+n,20001,n,'PERF252 paid chapter ' || n,'public','paid',100
FROM generate_series(1,300) n""")
smoke.pg("""INSERT INTO chapter_versions(id,chapter_id,version_no,body_markdown)
SELECT 3000000+n,2000000+n,1,'PERF252 isolated paid edition'
FROM generate_series(1,300) n""")
smoke.pg("""INSERT INTO chapter_publications(chapter_id,version_id)
SELECT 2000000+n,3000000+n FROM generate_series(1,300) n""")
smoke.pg("""INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
SELECT id,20000,'RUB' FROM users
WHERE email_normalized LIKE 'perf252-paid-buyer-%@tor-ci.example'""")
smoke.require(smoke.scalar("""SELECT count(*) FROM reader_wallet_accounts a
JOIN users u ON u.id=a.user_id
WHERE u.email_normalized LIKE 'perf252-paid-buyer-%@tor-ci.example'""") == buyers,
    "buyer wallet fixture count does not match")
smoke.require(smoke.scalar("""SELECT COALESCE(sum(a.share_percent),0)
FROM revenue_share_assignments a
JOIN revenue_share_states s ON s.book_id=a.book_id AND s.current_revision=a.revision
WHERE a.book_id=20001""") == 100,
    "paid book revenue shares must total 100%")
smoke.require(smoke.scalar("SELECT count(*) FROM chapter_publications WHERE chapter_id BETWEEN 2000001 AND 2000300") == 300,
    "paid edition fixture incomplete")
print(f"PERF-252 disposable paid fixture: {buyers} wallets, 300 chapters; no real funds")
