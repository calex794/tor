# DROP-07e — operator verification of PostgreSQL receipts against S3

The operator coordinator handles both directions. For PostgreSQL → S3 it
claims a separate checkpoint for SINGLE or PACK. PACK includes the archive
and its separate manifest. A cursor from the other namespace is rejected.
Each call checks up to four receipts under one 60-second deadline. The S3
adapter verifies actual bytes and SHA-256, with a maximum of 16 MiB per
receipt. A GET error, including a provider 404, is counted as unverifiable.
The aggregate counters are committed with the scope, cursor and page index.

The PostgreSQL acceptance test exercises the separate SINGLE and PACK
queries, pagination through the manifest and archive, and cross-namespace
cursor rejection. The disposable PostgreSQL and MinIO test runs the
coordinator against one exact archive and an unavailable manifest, then
reads the committed aggregate from PostgreSQL.

A completed scan can include uncertainty. Repeat independently before
comparing results, and verify saved editions and the backup inventory
before any retention decision. This coordinator only reads S3 and writes
audit checkpoints. It has no application startup, public route or DELETE.

## Closed PACKs and read admission

The expected scan retains both catalog receipts for each closed PACK. The
operator sees a separate `sealed` count as the page advances. Completed
expected reports persist that count in `protected`. These receipts are
counted without a provider GET or a successful verification. Observed
scans keep reporting registered keys when they are present in the bucket.

Before reading an open PACK or its manifest, the scanner reserves one
minute on the exact PACK placement gate and bounds its own attempt to 30
seconds from admission request through byte verification. The placement
seal serializes against that reservation. An unconfirmed admission is
counted as unverifiable. The dedicated database audit role needs EXECUTE
on `public.reserve_free_pack_inventory_read(text,text)` in addition to
its existing checkpoint access.

Independent backup verification can still rehash retained archive copies
using its restore-only credentials. It is a separate source of evidence.
