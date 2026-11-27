# RECOVERY-10e-b2: exact S334 encrypted purchase finance witness

The existing S334 logical restore fixture proves historical V1/V2
entitlements, actual encrypted storage, wallet debits and WebCrypto. The
first independent draft failed before backup: its two checkouts used the
default wallet-only PgCheckout, whereas its SQL required D-08 postings.

Publish explicit CI-only immutable finance terms (20% platform, seven-day
refund window) for the disposable S334 book and opt both actual checkouts
into the D-08 writer. These are synthetic test terms, not production policy
or funding. Each 175-minor-unit purchase must produce a 425 remaining wallet,
35 platform credit and 140 creator earning, with the original refund deadline.

Extend the already merged DEPLOY-07c read-only snapshot instead of adding
a competing query. Verify each debit, command, pin, posting and recipient,
and compare exact row evidence plus edition/registry hashes after restore.
Keep all seven canonical entitlement checks, negative revocation control
and source-unchanged assertion. No raw key material or plaintext is output.
The restored revocation insertion exercises access denial, not a financial
refund settlement. No runtime policy, production migration, deployment or
physical PITR schema change is included; full CI is required.
