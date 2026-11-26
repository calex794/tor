# AUTH-06b — password-confirmed S3 profile save

The existing admin S3 connection POST now requires the current administrator
password, a bounded attempt budget and the existing bounded password worker
before any external HeadBucket. The A-issued single-use proof is bound to the
live session, storage.s3.connection.save.v1 and length-framed SHA-256 of the
exact submitted bucket and credential pair. Raw password, keys and proof are
not logged or rendered. The storage repository consumes the proof atomically
with the encrypted singleton profile and audit; role revocation, session
revocation, different intent, expiry or replay deny the save. HEAD remains
fixed to Yandex HTTPS; no automatic activation or object mutations.

An additive SQLx migration extends the existing proof constraint by one
scope, preserving the four previous scopes and their five-minute maximum.
No account/storage crate dependency is introduced. The integration tests use
a synthetic encryption key on disposable PG, not actual Yandex credentials.
HTTP tests deny absent/wrong password before networking, storage PG tests
cover exact intent and single consumption, and RIL mutation tests require
password verification and consumption guards. The test proof fixture is
separate from the A issuer tests. No production deployment or provider call.
