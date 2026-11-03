# C-12 / S065 — Reader entitlement and bypass acceptance matrix

**State: partial, repository-only.** This inventory documents real evidence and open parity gaps. It is not production deployment approval or a claim of DRM/copy protection.

Source: [S065 scenario](https://app.notion.com/p/40432a84beec8115b1f9f8567f855ce7), [C-12 task](tasks.tsv), [reader API](api-v1.md), [paid delivery contract](../contracts/paid-delivery-v1.md). Reference implementation: NEwBOO `backend/app/web/routes/reader_page.py` and related history/preview routes; absent legacy endpoints in TOR are **not** equivalent to implemented feature parity.

## 03.11.2026 — immutable audit for privileged content deliveries

TOR records the **successful delivery** of privileged content, rather than merely the presence of the administrator reader shell. The real A session and active administrator role are rechecked under PostgreSQL `FOR SHARE` locks in the same transaction as the audit insert. A failed insert/commit suppresses the privileged response. The record contains only administrator ID, chapter ID, version ID, action (`unsealed_body`, `sealed_chapter_grant`, `sealed_asset_grant`) and timestamp. It never stores text, S3/object URLs, image IDs, encryption keys, cookies or session digests. Audit rows are immutable (no UPDATE/DELETE). Buyer and Layer grants cannot create an administrative audit. A revoked role cannot create new successful-delivery events.

Repository: `crates/commerce/src/repositories/admin_reader_audit.rs` and `migrations/202611039764_s065_admin_chapter_read_audit.sql`. The existing S065 PostgreSQL/Axum matrix checks successful events for each administrative delivery path, including the no-G reader, and rejects post-revocation events.

This is a repository-only security audit, **not** historical-reader parity, a deployed CDN audit or a production rollout. Existing short-lived signed bearers are not invalidated retroactively.

## 03.11.2026 — administrator drafts when paid G storage is entirely disabled

The default authenticated PostgreSQL runtime (`router_with_postgres_reading_auth`) and both authenticated cover-only variants now mount an **independent administrator-only reader**. It requires no private G delivery port, `TOR_PAID_GRANTS_ENABLED`, paid S3 bucket, object KEK or paid S3 origin. This is a current-unsealed B body inspection capability bound to a live administrator role and session, not an override for ordinary buyer paid entitlements.

- `/reader/{id}` redirects the active site administrator only if the current unsealed edition exists. Sealed paid editions **do not redirect** when no S3 browser is mounted; their regular paywall/non-disclosure behavior remains.
- The private shell has no plaintext/keys/S3 URLs, uses same-origin CSP, and the body POST demands Origin/CSRF, enforces rate limits and checks the current version and role again.
- The no-G router does **not** expose the paid chapter delivery grant, protected PNG grant or paid buyer reader. No fake signer, synthetic purchases, Layer grants or wallet entries are created.
- PostgreSQL/Axum acceptance exercises the actual no-G auth constructor alongside the existing API-only G and paid-browser modes, including free/paid unsealed drafts, forged access, account revocation and current-version switch.

Historical admin version browsing, media, moderator parity and explicit administration-reading audit remain independent S065 gates. Repository-only; not a deployment acceptance claim.

## 03.11.2026 — current unsealed administrator draft without private S3 browser

The existing API-only authenticated PostgreSQL router now mounts the strict administrator shell and the current unsealed draft POST, without enabling buyer paid HTML, paid checkout, Layer, signed S3 content, or any legacy history/preview/CDN alias. The public /reader/{id} redirects only an authenticated active site administrator when D confirms a current non-attached plaintext working draft. In this mode the HTML has an empty S3 origin, a same-origin-only CSP, no plaintext, and the JavaScript retrieves the current version through A session + Origin/CSRF + revalidated A/D and B. A sealed paid edition still returns 404 without the S3 browser. A revoked admin or outsider cannot use the route. The same PostgreSQL/Axum acceptance exercise runs both opt-in browser and API-only variants.

**Scope**: only current unsealed administrative reading when the S3 browser is disabled. Full historical revision preview, media, CDN invalidation, and production verification are independent S065 acceptance gates. No live server, finance, or DB migrations.

## Policy / route matrix

| Route or state | Positive proof already in main | Negative proof already in main | New C-12 proof in this PR | Limit |
| --- | --- | --- | --- | --- |
| Paid SSR `/read/paid/{chapter}?purchase_id=...` | Owned immutable V1 after V2 | Unpurchased author, another user, guest; refund/hold/delete | Actual PostgreSQL-backed administrator role, plus guest on the exact purchased ID | No server-paid body in HTML |
| Chapter delivery grant POST | D pinned V1, B attachment, G presigned encrypted GET | CSRF/Origin/tampering, stranger, refund, moderation, race | Admin session and guest denied with empty no-store response | Already-issued bearer valid until TTL; plaintext already decrypted cannot be recalled |
| Private image delivery grant POST | Owner-only exact asset of purchased V1 | Wrong UUID/version, stranger, revoked, hold | Admin and guest denied without invoking G signer | Real PNG/WebCrypto checked separately |
| Public paid `/reader/{chapter}` | Public price/paywall metadata | Cannot inject `purchase_id` or `preview=1` to obtain paid text | Admin and guest with forged query get paywall only | Public metadata is intentional |
| Legacy history, preview, versions API, media, CDN | None: these routes are not implemented in TOR | Existing routes deny 404, no ciphertext/keys/text | Guest and admin explicitly tested against representative aliases | 404 is a closed bypass, **not** history/preview parity or deployed CDN purge |
| Purchased progress GET/PUT | Buyer reads own exact V1 state (never body) | Foreign actor cannot read/overwrite by stolen purchase ID | Admin cannot read owned progress by privilege alone | No entitlement inferred from stored cursor |
| Free reader | Published public free edition | Draft, held, deleted and paid body unavailable in public reader | No new proof here | Covered under S005; not replaced by S065 test |
| Purchase refund / 90 s S3 TTL | Fresh grant after refund denied; issued ciphertext URL expires | Signing/refund race rechecked by D | No new proof here | No immediate invalidation of an already issued S3 URL |
| Author upload / purchased image | Real PG/MinIO PNG in purchased V1, 1440/390 Chromium | Outsider/tampered/expired | No new proof here | Single-image author workflow is not full arbitrary inline image parity |

## Reproducible acceptance evidence

- Real PostgreSQL/Axum role checks, no mocked identity: `crates/web/tests/paid_grants_http_pg.rs` and `crates/web/tests/paid_grants_http_pg/s065_role_matrix.rs`. The latter inserts an actual active `account_roles.administrator` in **disposable `tor_ci`**, verifies it via `session_actor`, cleans up the temporary active role even on assertion failure (so subsequent A-05 last-admin checks remain isolated), then sends valid-origin/CSRF requests.
- Exact image purchase/MinIO/WebCrypto: `crates/web/tests/paid_grants_s3_pg.rs` and `tools/s034_single_paid_browser.cjs`.
- Historical aliases/progress/buyer/author: `s065_alternate_reader_access_matrix` in `paid_grants_http_pg.rs`.
- Previous merged [PR #426](https://github.com/calex794/tor/pull/426): [post-merge integration 16/16 SUCCESS](https://github.com/calex794/tor/actions/runs/41510027811), [repository rules SUCCESS](https://github.com/calex794/tor/actions/runs/41510027954).
- Current C-12 administrator-role addition is **not yet accepted** until exact-head PostgreSQL runtime CI, repository rules and post-merge regression all succeed.
- CI command: `cargo test -p tor-web --test paid_grants_http_pg -- --ignored --test-threads=1` in `.github/workflows/tor-integration.yml`; run only against disposable `TEST_DATABASE_URL=.../tor_ci`.

## Remaining acceptance blockers (do not mark S065 complete)

1. Map every **actually implemented** history/preview/versions/media route from NEwBOO to its intended TOR behavior; missing routes are safe by absence but do not prove user-facing parity.
2. Verify moderator / team actor distinctions independently where those roles have different capabilities; the site administrator must not become a paid entitlement by default.
3. Explicitly document deployment-dependent public origins/CDN invalidation and short-lived signed S3 URL semantics. No current repo-only test proves a deployed production CDN or revocation of already delivered plaintext.
4. C-12 requires a separate signed-off comparison for S005/S006/S007 as well as S065. This file proves one additional role slice, not 4/4 line C or an increment of the canonical 20/60 scenarios.

**Operational constraints:** no deployment, external payments, feature-flag activation, permanent DB changes, migrations, or protected-content logs.