# D-06 — read-only purchase/term use-evidence audit (Draft)

## Why this exists
Checkout grants the paid right to read a protected chapter or a Layer term.
It is **not proof of use**. A server first-grant issuance is not receipt or
decryption. A browser render acknowledgement is a positive client assertion,
**not a cryptographic proof of human attention**; a lost or suppressed ACK is
possible. An empty history is **UNKNOWN**, especially for pre-cutover purchases.

## Single-statement, actor-bound observation
`PgRefundUseEvidence::direct(actor, purchase)` checks the exact canonical
buyer/purchase, immutable wallet debit, pinned chapter/version and share
snapshot. `PgRefundUseEvidence::layer(actor, term)` checks the exact term,
actor and D-05 wallet provenance. Both read the following evidence in one
consistent PostgreSQL MVCC SELECT, without writes, row locks, bearer URLs,
plaintext, or refund decisions:

| Evidence | Meaning | Can authorize ordinary refund? |
| --- | --- | --- |
| `BrowserRenderReported` | A validated browser reported rendering its chapter | **No** — positive client claim, not a money decision |
| `ChapterTextGrantUnconfirmed` | Text-specific protected S3 grant was issued; browser receipt absent | **No** — delivery/read status uncertain |
| `ProtectedGrantOnly` | Some protected grant was issued (e.g. image/other chapter), not chapter text | **No** — no proven chapter open |
| `Unknown` | No recorded signals, including possible legacy/uninstrumented use | **No** — absence is not proof of unopened status |

`access_revoked` is a separate boolean and does **not** imply a settled
refund. An unknown/foreign/not internally paid identifier returns
`NotFound` without showing foreign evidence. Corrupt marker ordering fails
closed with `Unavailable`. Direct and Layer identities never fall back.

## Enforcement boundary
The read-only audit is advisory for humans and tests; it **cannot** claim a
refund is eligible, nor may a later money writer use its stale response.
Existing `LayerRefundService::preflight` continues to return
`PolicyUnavailable` for genuine terms with no approved immutable
purchase-time refund policy/deadline, or `RevocationPresent` when access was
revoked. The new `LayerRefundService::observe_use` exposes evidence
**separately**, with no `refundable` field, no HTTP route and no writes.

Future refund approval/reversal must verify source-specific immutable policy
and exact deadline captured **at checkout**, plus ownership, wallet,
first-grant and browser evidence under the SAME purchase/term lock **inside
one SERIALIZABLE financial transaction**, with repeatable idempotency,
audit/outbox, and FIN-B05...B11 before activation. A grant without receipt,
legacy with missing markers, or network-loss dispute must not be auto-refunded
or automatically rejected on this audit alone.

## Integration status and constraints
This Draft stacks after #237 and assumes its exact C-06 evidence schema; it
adds **no migration**, so does not create another SQLx version collision.
I-14 Draft integrates merged S035 #224 at migration
`202610290003_batch_chapter_purchases.sql`. The Draft #216 snapshot is
renamed **only here** to `202610300017_layer_refund_policy_snapshot.sql`;
this does not prove the old version was never applied to any retained DB.
Before production, audit installed `_sqlx_migrations` and apply a distinct
compatibility plan if needed. Passing an isolated integration test does not
constitute release approval.
No policy defaults, payments/refunds, creator settlement/payout, feature flags,
merge to main or deployment are authorized.
