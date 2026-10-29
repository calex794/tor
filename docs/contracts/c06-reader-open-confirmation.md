# C-06 — paid access, issued grants and reader opening (Draft)

## Product rule
The product sells **access to an individual protected chapter/version**,
not ownership of a book. Successful payment creates an entitlement, **not use**.
Use starts on the first actual opening of the purchased protected chapter.
Batch purchases are independent per chapter; Layer renewals are independent
terms and a future term must not inherit a previous term's opening.

## Three independent observations
1. **Paid entitlement:** the immutable chapter purchase / active Layer term.
2. **Grant issued:** `chapter_purchase_first_exposures` or
   `commerce_layer_term_first_exposures` was committed before returning S3
   URL/DEK. Network loss after that commit is possible: issuance is **not
   proof** that the browser received or opened the text.
3. **Render acknowledgement:** after the browser completes S3 GET, verifies
   ciphertext SHA-256, AES-GCM, AAD and plaintext SHA-256, and inserts the
   protected text into the DOM, it POSTs
   `/api/v1/reader/chapters/{chapter_id}/open-confirmations` with a CSRF
   token. D locks the exact owned purchase or active Layer terms, rechecks
   a **chapter-text-specific** grant for that purchase, chapter and pinned
   version (a previous image grant or grant for another Layer chapter does
   not qualify), rights, and atomically appends an immutable first
   render confirmation. Checkout, HTML shell, invalid rights, download
   failure and decrypt failure never generate this ACK.

**Limitation:** the ACK is a browser's claim about verified rendering, not
cryptographic proof of actual attention. A malicious client can withhold it.
A lost ACK after successful rendering is possible. No telemetry can by itself
prove exactly when a human read the content.

## Future refund triage (no money implementation in this PR)
- Confirmed render event: product evidence of first opening; ordinary
  refund policy can treat the purchase as used.
- Grant issued but render not confirmed: **UNKNOWN / manual review**, never
  auto-classify as unused or as proven read.
- No grant or ACK since a verified cutover: potential unopened purchase,
  subject to the purchase-time refund policy.
- Pre-cutover legacy without markers: **UNKNOWN**.

No automatic refund is enabled. A future ordinary refund approval must recheck
ownership, deadline, entitlements, issued/confirmed evidence and immutable
ledger provenance under the same purchase/term lock in one SERIALIZABLE
reversal transaction. Defects, failed delivery, chargebacks and T&S actions
are independent processes, not ordinary refund bypasses.

Additive migration: `202610290007_paid_reader_render_confirmations.sql` (I-14 integration-tree name; the original Draft #237 still uses `202610290006`), which also creates separate immutable `chapter_purchase_text_grants` and `commerce_layer_term_text_grants` to prevent image-first / other-chapter-first false confirmations.
Stacked Draft on #231. No backfill, feature flag changes, checkout edits,
wallet/refund/payout postings, main merge or production deployment.
#235 separately coordinates collisions with other pending migrations.
