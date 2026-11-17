# DROP-07h — exact FREE key fencing and quarantine

Publication reserves a private bucket/key durably before object writes.
Both PACK components are reserved in one PostgreSQL statement in the same
order. PostgreSQL also fences new SINGLE receipts, PACK receipts, direct-PACK
stage receipts and R4a pack specifications. This catches older code attempting
to attach after a key was quarantined. The database is not held during S3 I/O.

An operator starts quarantine with a positive, exact, non-null object version,
ETag, size and full SHA-256. PostgreSQL claims the same unique bucket/key and
checks all preexisting receipts and pending references in that transaction. A
pending PACK sibling is enough to protect the object. The winner's fence is
immutable, including after a lost caller response. Rechecks require at least
24 hours of quarantine, the same exact primary version, a versioned backup
locator in a separate bucket and a fresh reference check. Matching retries
record only one immutable event.

Published, reserved, pinned and rollback source editions stay protected.
A crashed writer's durable reservation is intentionally not aged out. The
initial cleanup scope covers unregistered objects that were never reserved;
retiring an existing write reservation is a separate lifecycle action with
its own proof and must not be inferred from a LIST report.

The repository only records quarantine and rechecks. The operator path must
read both versions and compare actual bytes before recording a recheck. No
HTTP route, cleanup timer or bucket mutation follows from an audit record.
