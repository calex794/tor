# AUTH-04a / A-03, A-05 — administrator revocation authority

The owner requested independent revocation by the user and administrator.
This slice provides the A-owned PostgreSQL command and its atomic audit;
it deliberately does not expose an HTTP route yet.

The administrator is derived from the session digest, then revalidated
under the existing role-authority advisory lock and ordered account/session
locks. A fresh VerifiedPassword must match the locked administrator
credential. Select a canonical target session UUID or all target sessions.
The target is a different existing account; self-service remains on the
existing account-security path. The operation never revives expired or
revoked sessions, nor changes the target's roles or credentials. Subsequent
password-authenticated sign-in is allowed unless the account is suspended.

The additive immutable audit contains administrator ID, target ID, optional
selected session UUID, actual revoked count and database time. No credentials,
IP, raw cookie or token digest. Zero-row retries do not insert audit events.
The audit insert and session revocation share one transaction. Migration
identity must be checked against concurrent integration work before merge.

The existing accounts access_audit_pg CI suite tests exact session and bulk
revocation, foreign isolation, active-role enforcement, retry, and immutable
audit. Browser/admin form, CSRF, bounded lookup, audit presentation and
concurrency/load acceptance are AUTH-04b/05 follow-ups. No VPS deployment.
