# S020/#486: Layer CURRENT paid-version age gate

NEwBOO access policy is the behavioral reference. Purchased Layer terms are not age exemptions.

- D-05 `PgLayerChapterEntitlements::resolve_entitlement` reads the current Book and actor's account in **one SQL statement**. Require active account, `basic` or explicit adult opt-in or trusted active pro_adult grant. Legacy NULL fails closed and a Book-level site `blocked` denies.
- Preserve current published paid Chapter, release time, holds/deletion, term expiry, term revocations and the historical Layer version graph. A blocked/hidden *Layer storefront* does not silently cancel an already paid term.
- `pending_deletion` does not revoke a paid Layer term while its current chapter is published. Its exclusion from anonymous catalog is separate. Site `blocked` and Book hold still deny.
- Existing D-05 PG test exercises adult, pro_adult, revocation, NULL, pending deletion, site blocked and restored current published V2; shared `setup` fixture is explicitly basic.
- Layer GET shell and POST encrypted body/asset grants already route via the same D entitlement authority, before and after G signing. Dedicated live S3 HTTP regression and admin exemption remain full #486 gates.
- No new DB migration; only repository code and disposable CI tests. No production deployment. Strict S020 24/60 remains Partial.
