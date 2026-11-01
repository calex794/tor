# C/D-06 — Layer first protected-grant exposure (Draft)

**Scope:** C/S3 Layer-credential issuance only; read-only refund preflight remains closed. Neither a refund policy nor cash reversal is approved by this work.

## Enforced boundary

- HTML Layer shell, table of contents, checkout, metadata requests, failed authentication/CSRF/entitlement, failed B edition resolution and failed G signing do **not** count as paid exposure.
- After C has signed and assembled a successful encrypted chapter **or image** grant, but before the bearer URL/DEK is returned, D revalidates the exact current published chapter and all contributing **active** Layer terms under PostgreSQL row locks. A fresh READ COMMITTED query rechecks revocations after any lock wait.
- The first issued grant creates immutable \`commerce_layer_term_first_exposures(term_id, chapter_id, version_id, object_kind, granted_at)\`. Concurrent/replayed grants use \`ON CONFLICT DO NOTHING\`; the first timestamp and edition cannot change.
- A deferred 30-day renewal has not started: no event for P2 when reading on P1. If two actively valid purchased Layers independently grant the same Book, both receive a conservative evidence marker. It does not revoke either term, reduce a wallet balance or create earnings.
- Direct purchased chapters are not automatically marked by this **Layer-specific** migration; default \`source\` fallback to Layer is marked. Direct first-read must have its own audit before the broader refund authority can be activated.

## Non-equivalences and release gates

1. \`granted_at\` is **a credential issued before HTTP response**, not proof the user's network successfully fetched/decrypted S3 ciphertext. This distinction requires legal/product policy approval before it affects refund eligibility.
2. A missing marker on old or newly cut-over terms **never proves unopened**. Previous delivered URLs cannot be recalled, and a network interruption after commit may record a grant never received by the client.
3. Future D-06 approval/reversal **must lock the same term rows** within one serializable financial transaction, then recheck policy, deadline, paid exposure, purchase and exact owner. This PR adds no refund decision, reversal or wallet movement.
4. Financial/legal owners still need to approve policy identity/clock, legacy purchase handling, multiple active entitlements and direct versus Layer exposure. This event does not grant a refund right.
5. Real PG negative/concurrent tests and S3 authenticated grant test must pass on the final SHA. No merge to main, production deployment, or payment/refund flags from this Draft.
