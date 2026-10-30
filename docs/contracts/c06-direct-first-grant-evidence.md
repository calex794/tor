# C-06 — first protected grant for direct purchased V1 (Draft)

**Technical issuance evidence only, not an approved refund policy.**

The existing C/D-06 Layer-grant evidence in stacked Draft PR #228 tracks Layer terms. This change covers the missing directly purchased pinned chapter version V1, including an explicit \`purchase_id\` and the API's default direct-first selection.

## Contract

- C assembles the encrypted chapter or private-image grant (D → B → G → D), then before returning a signed S3 URL/DEK, D verifies the authenticated buyer's canonical immutable purchase and still-readable pinned version under a PostgreSQL row lock. A fresh READ COMMITTED query checks rights after any conflicting revocation.
- The composite foreign key of \`chapter_purchase_first_exposures\` binds the event to the same purchase, buyer, chapter and version. \`purchase_id\` is the primary key. Parallel/replayed grants do not replace the timestamp or original content kind; UPDATE/DELETE are forbidden.
- HTML reader shell, checkout, metadata, failed CSRF/session, missing rights, failed B/G signing and wrong object/version do not mark an exposure. The source choice cannot switch a direct paid purchase to Layer without D's canonical check.
- Direct purchases remain readable after a storefront hide but not after a moderation hold/deletion/refund. Direct and Layer entitlements continue to be separate; a separate future renewal never inherits a previous term's event.
- Real PostgreSQL, HTTP and private MinIO E2E test cases exercise negative access, immutable event, duplicate/concurrent requests, legacy no-marker behaviour, default source, and no wallet movements.

## What is NOT proven

- An issued grant **does not prove the client received, downloaded, decrypted or read the content**; a network failure after DB commit is possible.
- A missing event for pre-cutover historical purchases means **UNKNOWN**, never proof of non-consumption or of refund eligibility. Old pre-signed bearer URLs may work until expiry.
- Finance/legal must independently approve the refund clock, policy version, scope and first-access semantics. A subsequent serializable refund/reversal must lock the *same purchase row* and revalidate the authoritative evidence under that lock.
- No checkout/policy/ledger/chargeback/payout changes, no feature flags, no main merge and no deployment from this Draft.
