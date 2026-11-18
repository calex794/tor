# DROP-07i — operator proof and retirement drill

`cargo run -p tor-web --features s3-free --example drop_quarantine -- enter KEY VERSION`
reads the named immutable version, checks bounded actual bytes and records a
PostgreSQL fence. Use the approved operator database, primary bucket/endpoint,
region and scoped credentials. The example reads these from
`TOR_DROP_AUDIT_DATABASE_URL` and the `TOR_DROP_PRIMARY_*` environment variables.

After 24 hours, `recheck KEY VERSION BACKUP_VERSION` rereads the primary
and corresponding exact archived object under separate `TOR_DROP_ARCHIVE_*`
configuration. It requires equal SHA-256 and size, a distinct bucket, and the
same original version and ETag. PostgreSQL checks references again before
appending evidence. Configure the archive on an independently controlled
account with verified recovery and version retention.

`retire-drill KEY VERSION` runs the physical cleanup exercise only on
loopback, in `tor-gc-drill-*` buckets, with a disposable database. It rereads
the retained copy, verifies the target, and records a durable request before
sending a version-specific conditional DELETE. Only a positive
`NoSuchVersion` on that exact version settles a request. An interrupted
operator can resume the recorded request. A missing version without a
request or an ambiguous provider error remains unconfirmed. The permanent
key fence blocks any later app attempt to publish it.

The acceptance fixture creates two versions under the same key and exercises
a crash after the exact-version DELETE but before the PostgreSQL receipt.
The newer version, another object and the archive copy must survive. A
separate Yandex probe and independent off-site recovery are prerequisites
before making a real provider deletion path available.

## Registered PACK placement gate

The catalog owner can freeze a replaced PACK against new placement writes
with `seal_replaced_free_pack_placements(pack_id)`. It requires a nonempty
bounded membership. Every source edition must have an active placement in
a different open PACK in the same bucket with the same content digest and
size. This also preserves editions with no saved or Layer pin yet, since
future pins refer to the edition rather than its former PACK.

The seal uses `READ COMMITTED` and locks the source gate and current
edition pointers with `NOWAIT`. A competing placement write yields a
retryable lock error; a missing or different replacement yields `false`.
A committed seal remains in PostgreSQL and a repeated call returns `true`.
Afterward the normal placement triggers reject both new source members
and promotion of old source candidates. The runtime role has no direct
write access to a gate or permission to call the seal.

This catalog operation keeps both registered source receipts and has no
provider side effects. The separate retirement process still needs exact
replacement bytes, independent recovery evidence, read-grant drainage,
and provider acceptance before either source key can be deleted.
