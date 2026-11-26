# AUTH-08a — session continuity through real release and database drills

Extend the existing mandatory S061 and I-11 PostgreSQL acceptance, rather
than creating an alternative deployment path. Three synthetic cookies are
seeded for the retained reader in each disposable fixture: a live
six-day remaining legacy session, one revoked session, one expired
session. Only SHA-256 digests go to PostgreSQL; the private temporary
witness is removed by the existing restore cleanup.

S061 checks the same cookie values on the old slot, candidate slot, after
promotion and after crash rollback. In the distinct-binary run, it also
checks the old server after additive migrations. Passive /me requests
must preserve session identity and expiry; rejected cookies must not
receive a session cookie. Report stages only, not credentials.

I-11 checks the unchanged restored session and login facts, verifies
actual authenticated HTTP with the restored signed binary, renews the
live legacy cookie to 30 days on a user navigation, and verifies denial
of both revoked and expired sessions even on qualifying navigation.
The original database must retain its original session snapshot.

These are synthetic persisted session fixtures, not a new real login
flow or a physical WAL replay test. Mandatory S061 normally uses the
same candidate binary in both slots; distinct-version rollback requires
the existing FND-06 two-binary run. A backup contains revocations only up
to its recovery point. Revocations after that point require WAL/PITR
or a deliberate invalidation policy; no claim that a stale backup can
preserve later security events. No production deployment.
