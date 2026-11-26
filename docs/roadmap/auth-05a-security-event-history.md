# AUTH-05a / A-03, A-07 — account security event history

Successful registration, password login and password change already create
canonical account_login_events atomically. Keep those rows as the source;
do not copy them or reconstruct historical actions.

Add a bounded owner-visible security history for newly ended sessions
(including normal logout), individual owner revocation and password reset.
Every new audit fact is inserted in the same statement/transaction as the
corresponding mutation. Failed commands and zero-row revocation retries do
not record duplicate successes. Password reset records completion even if
there are no remaining sessions. The revoked_count is the number of rows
actually changed, not an estimate of online devices.

The new history query derives its owner from a live cookie digest in the
same SQL snapshot, never accepts a caller-supplied account ID, and combines
the latest 50 real events from the existing login table and new security
table. The browser-facing DTO has no IP, session ID, digest, password,
reset token, or raw user agent. A read-only transaction and pool/statement
timeouts bound the query. It returns no events for an invalid session.
The 90-day retention horizon matches existing login history; old rows are
removed by the existing batched cleanup. Updates are forbidden, but this
is a retention-limited personal history, not the immutable admin audit.

This independent slice is based on main, not the unaccepted AUTH-02/03/04
branches. Integrate the owner_revoke_others event into AUTH-03 and project
the existing separate administrator audit into the owner timeline once
AUTH-04 is accepted. Account state/role events remain in their own existing
journal; failed login observations retain their separate 7-day policy.
The owner UI and administrator history presentation are subsequent slices.
No new HTTP route or production deployment.

Acceptance: existing slices_pg suite covers audited login, single owner
revoke, logout, repeated/failed commands, reset, revoked-cookie rejection,
foreign isolation, bounded retention and update rejection. Integration CI
and browser/recovery acceptance are still required.
