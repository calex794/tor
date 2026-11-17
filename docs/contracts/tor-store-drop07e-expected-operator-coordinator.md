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
