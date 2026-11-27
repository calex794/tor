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

## AUTH-08b — stale backup revocation gap and recovery policy

After the normal continuity and source-unchanged assertions, the isolated
I-11 drill deliberately revokes exactly one synthetic source cookie.
The already restored archive cannot contain that later revocation; its
loopback HTTP process must demonstrate that the old cookie still works.
Then invalidate all live sessions in the disposable restored database,
verify zero live sessions and a zero-row retry, and exercise denial of
all three old cookies on both passive and qualifying active navigation.
New synthetic sessions must still work, and the invalidation must not
mutate the source. This is a test of session invalidation, not a real
password login or an independently reviewed operational recovery.

For a normal release/restart with unchanged PostgreSQL, retain sessions.
For point-in-time or backup recovery that may have lost security events,
keep TOR offline and the recovered database isolated until security
state is reconciled. If continuity of revocations cannot be independently
established, require explicit invalidation of all restored live sessions
before granting access. A stale recovery can also lose password changes,
account closures, role removals and reset-token consumption; invalidating
sessions alone does not establish that the recovered identity state is
safe. Reconcile those separately before public cutover. This policy must
be wired into the separately approved operational recovery gate, not
executed automatically on normal startup, migration or deployment.

The fixture intentionally starts a loopback-only HTTP server to prove
the risk before invalidation; never expose a real stale restored database
to traffic for that experiment. No external restore, promotion or VPS
changes are authorized by this test.
