# G-09 upstream MinIO response experiment (#533)

Temporary branch only. The mandatory storage-s3 workflow and its strict
first GET test are unchanged. No production code, credentials, VPS,
persistent database or external storage. No merge intended.

Known upstream issue minio/minio#21633, fixed by #21651. The existing
September binary predates that fix. Three arms: verified existing
RELEASE.2025-09-07T16-13-09Z, immediate source parent
c6d3aac5c436d8468cf5a807498b6a97db421979, fixed source
52eee5a2f19ea8ef06bf2dd5ab64ab41424cb591. Build the latter two with
Go 1.24.8 and one unchanged TOR test executable for all arms.

For each fresh loopback server, run 10000 isolated cycles with ordinary
connection pooling, and 10000 with Connection: close. Same SDK retry
configuration (one attempt) and same immutable PUT/conflict/first GET
sequence. Each target has a unique canonical object ID. No intervening
probe. Count every first Unavailable permanently, classify through the
existing safe diagnostics, then independently verify HEAD and actual
bytes/hash. Any wrong bytes, failed write/rejected-overwrite contract,
failed integrity probe, other error or incomplete sample fails the
experiment. A completed measurement with first failures is not a pass
of the mandatory G-09 gate. No raw packet/HTTP/credential capture.

The immediate parent/fix comparison isolates the upstream code change
better than release/fix alone. A single clean sample is not proof;
record all arms and repeat independently if necessary. This initial
experiment addresses the quiet reproduction only, not concurrent load.

First independent run (45462153592): release 1/1000, parent 3/1000,
fixed 0/1000 pooled; all close controls 0/1000; 6000/6000 integrity.
The fixed sample completed but the shell parser incorrectly rejected a
libtest-prefixed line. Second run validates exact sample sizes, both modes
and counter arithmetic, uses reverse arm order and 10x larger samples.
