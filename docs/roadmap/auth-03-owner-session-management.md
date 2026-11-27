# AUTH-03 / A-03 — owner session management

Extend the existing authenticated GET/POST /me/security/sessions interface.
The bounded list keeps the current session first and shows the authoritative
expiration in UTC alongside its creation time and browser family. It does
not expose token digests, IP addresses or claim to show last activity.

The existing POST accepts session_id=others in addition to an exact UUID.
The explicit bulk form requires the current password, matching CSRF cookie,
same-origin policy and the existing account-scoped rate limiter. The server
derives the owner from the live requesting session. The database locks the
active user and verifies the exact password hash, then locks the requesting
live session before revoking every other live session of that owner, with
no dependence on the 50-row display cap. A repeated request is idempotent.
A newly authenticated device after the transaction may sign in normally.
No schema, endpoint or deployment changes.

Acceptance: real PostgreSQL HTTP A-03 coverage includes 53 active sessions,
the current device outside the natural newest-50 window, an independent
foreign account, CSRF/Origin/password denial, mass revocation beyond the
UI cap, current session preservation, retry and old-cookie rejection.
Browser coverage, security-event audit and administrator revocation remain
separate acceptance slices. No production deployment is authorized.
