# S020 / #486: anonymous Shadow and REC age gates

Reference: NEwBOO `book_indexability_policy.py` and merged TOR PR #497.

- Shadow Book and chapter *metadata* views require active site moderation, `basic` Book tier, no hold/deletion, and a lifecycle other than `pending_deletion`; Shadow additionally requires explicit `shadow_enabled`. A paid chapter can remain in the metadata view without revealing its body, price, or storage key.
- REC general-audience Books remain `basic`-only and exclude pending deletion.
- Unknown legacy `books.access_level=NULL` is not silently promoted to `basic`.
- Dedicated PostgreSQL CI verifies public Book+chapter+REC inclusion, adult/pro_adult/NULL omission, live age revocation, and pending deletion. Migration smoke marks its intentionally public fixture `basic`.
- No finance/purchase entitlements, S3 objects, version pointers or site admin read policy are modified.
- **Still P0 open:** session-aware library/follows, paid V1/V2/Layer/S3, full admin matrix. No production deployment/migration; S020 Partial and accepted 24/60 unchanged.
