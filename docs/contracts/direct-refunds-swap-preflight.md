# D-06 direct chapter refund and swap: fail-closed foundation (31.10.2026)

Source: NEwBOO \`refund_service.py\`, \`refund_posting_atomic_repository.py\`,
\`swap_posting_service.py\`, \`async_swap_posting_repository.py\`.
See \`docs/contracts/refunds-payouts-v1.md\` and PR #216 (Layer-specific draft).

This increment deliberately exposes **no refund/apply HTTP route** and
**no wallet/revocation mutation**. It adds:

- Nullable version/deadline fields on immutable *direct* purchase facts. Both
  must be captured in the purchase INSERT; old purchases cannot be backfilled.
  No approved policy exists, so checkout leaves both fields NULL.
- Unique, actor-scoped refund intent schema with one pending/processing request
  per purchase, immutable intent identity and guarded state transitions.
  There is not yet a request-creation endpoint.
- A read-only \`PgDirectRefundPreflight\` that verifies canonical purchase
  ownership, frozen terms, deadline, exact original wallet debit, revocation
  and open requests; it **always refuses to claim "unopened"** while the
  trusted first-grant barrier remains unmerged.
- A read-only direct chapter swap quote for a published paid chapter in
  **another book of the same owner**, aligning with NEwBOO's web form rather
  than the more permissive Python API. Quotes use integer RUB minor units;
  positive difference requires sufficient buyer balance. Preview is never
  permission to post money or rights.

**Unresolved blockers** (must not be silently inferred):
1. Finance/legal approves a versioned policy; checkout binds it at purchase,
   not retroactively. Historical NEwBOO uses seven days, but TOR has no
   approved active policy or migration backfill.
2. C's first protected S3 grant evidence must be integrated on current main
   and serialized with D's refund decision; missing evidence = UNKNOWN.
3. Implement trusted moderator request/decision, canonical financial reversal
   of buyer/platform/creator ledger, immutable allocations, entitlement
   transition, reacquisition and failure/retry behavior, all one PG commit.
4. Swap execute requires transaction-time target price/version/status, frozen
   owner/split, exact old/new allocation and idempotent transfer posting.
5. Real HTTP/PG/MinIO and concurrency tests; external funding/payout remains
   separately gated.

Validation for this increment: disposable \`tor_ci\` SQLx migrations and
\`cargo test -p tor-commerce --test slices_pg d_06_ -- --ignored --test-threads=1\`.
No server deployment. Do not merge as a completed refund feature.


## New private request store (no public endpoint)
\`PgDirectRefundRequests::request_once\` is a D-owned pending-intent writer,
not refund approval. It locks canonical buyer purchase, requires exact buyer
wallet debit and immutable B-10 share evidence, enforces one open request,
and deduplicates by actor + 43-character key + SHA-256 of the exact payload.
Replay returns the original request without changing the wallet.

Only a purchase-time policy version present in the immutable, **initially
empty** \`finance_direct_refund_policy_catalog\` can qualify. The seven-day
NEwBOO period was NOT silently ported or activated. A future approved
catalog entry must be effective before the purchase and its saved deadline
must not exceed the approved window. Existing checkout still stores NULL.

The real \`tor_ci\` tests include a deliberately SQL-seeded hypothetical
approved-policy purchase to verify request deduplication; this is a test
fixture only, not an operational policy or a new checkout path. Real
checkout remains strictly PolicyUnavailable. No refund money, creator
reversals, paid-first-grant recording, swap execution or HTTP routes exist.

## C-06 first protected grant (reused from #231)

The direct purchase first-grant code and schema are taken from the existing
[PR #231](https://github.com/calex794/tor/pull/231) rather than designing a
second mechanism. Its D-owned `PgDirectFirstExposure` stores exactly one
immutable row after successful encrypted grant assembly (both chapters and
assets). The live HTTP handler returns no bearer URL if the tracking commit
fails or the purchase is concurrently revoked. Extra grants cannot reset the
first exposure timestamp. The request creator takes the same purchase lock
and checks for first exposure using a fresh statement after lock acquisition.
A known exposure rejects new refund requests; absence remains UNKNOWN for
pre-cutover/historic purchases and never enables a finance approval.

**Integration note:** the stacked #231/#237 branches contain overlapping
files/migrations and must drop the duplicate direct slice when rebased to
`main`. This PR does **not** import Layer grant logic or browser-render
confirmation. The two mechanisms serve different duties: a first issued key
is the conservative refund barrier; render confirmation is optional analytics.


## Private moderator reject (31.10.2026)

`PgDirectRefundDecisions::reject_once` provides **reject-only** terminal
processing for pending requests. It locks active global administrator role
and user account, then locks the requested row and compares a monotonically
increasing `revision`. A simultaneous identical reject returns the original
receipt; conflicting reason/actor or stale revision is denied. Terminal
request rows cannot be restored to pending. The immutable buyer request
reason is separate from the moderator's `decision_reason`, and each decision
has an auditable `decision_reference`. No credit, charge, revocation or
creator posting is performed. This is deliberately an internal D repository:
**it is not mounted on HTTP**, and admin step-up must be integrated prior to
any future web route. Approval remains disabled until original buyer/creator/
platform ledgers, first grant proof, refund terms and a fully atomic posting
are verified. Tests exercise two simultaneous moderator rejects on `tor_ci`.


## Immutable first-grant cutover for single and batch checkout

`chapter_purchase_facts.first_grant_tracking_version` is NULL for every
historic purchase. Both single and batch buyer checkout bind
`c06-first-issue-v1` at immutable purchase creation, only when C's protected
HTTP issuance also records the first grant. Without this marker, an absent
first-grant row is UNKNOWN and new refund requests fail closed. With this
marker and an approved purchase-time policy, a missing first grant permits
request creation, **not** payment approval. Read-only preflight returns
`FirstGrantNotSeenInTrackedPurchase` as a non-financial state. Seller and
platform ledgers still require independent financial evidence. New tests
cover direct and two-chapter batch checkout tracking.

## Migration onto D-08 (2026-10-31)

The independent D-06 database schema now uses migration 202610310270
and C-06 first protected grant migration 202610310271. There are no
conflicts with creator follow 202610310190 or D-08 posting 202610310246.
Unapproved finance terms may exist for tests but refund requests remain
fail-closed without a separately signed/approved legal policy.

The borrower must have an immutable D-08 finance snapshot with original buyer
debit, platform credit, and matching author earnings. This blocks historical
wallet-only purchases, even if their grant rows are missing.
Approval and creator reversal are still deliberately unavailable.


## Moderator step-up before direct refund approval (2026-10-31)

Direct refund approval now requires a fresh password-verification capability
before a proof can be issued. The capability is created only by
`verify_password_for_step_up` and must be checked in the bounded web
`PasswordJobs` worker; raw passwords never enter the commerce repository.
Issuance rechecks the exact stored password hash, active global administrator
role, active session and pending request revision under PostgreSQL locks.

The opaque proof token is never stored or returned to the browser. Only its
SHA-256 digest is stored on the existing session row. It is bound to
`finance.refund.approve.v1`, the current session, request ID, expected
revision and normalized decision-reason digest; it expires after five minutes.
The approval transaction validates and consumes it atomically with buyer
credit, platform/creator reversals and entitlement revocation. Missing,
expired, reused, wrong-session, wrong-request, stale-revision or changed-reason
proofs fail closed with no business writes. Session revocation, password
rotation and administrator demotion invalidate the proof.

The approval method remains internal and is not mounted on HTTP. Any future
web/API handler must perform same-origin + CSRF validation, authoritative
administrator lookup, account-scoped attempt limiting and bounded current
password verification before issuing the proof. No user-facing refund request
or approval flow is enabled by this change.
