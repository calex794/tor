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
