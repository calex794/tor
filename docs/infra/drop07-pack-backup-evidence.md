# DROP-07 — backup evidence for one PACK

Use an isolated restored PostgreSQL snapshot and the independently retained
object manifest and SHA-256 for its source bucket and recovery point.
Recover the exact PACK and its sidecar with restore-only credentials.
In an absolute staging directory, keep their full object keys as relative
paths. Stage both files by themselves. The retained manifest can cover other
objects from the same source bucket.

```bash
set -o pipefail
psql -Xq --no-password --dbname "$TOR_DR_RESTORE_URL" \
    -v bucket="$TOR_DR_SOURCE_BUCKET" -v pack_id=123 \
    -f tools/drop_restore_pack_receipts.sql |
    python3 tools/drop_verify_restore_receipts.py \
        --manifest "$TOR_DR_PRIVATE_MANIFEST" \
        --expected-manifest-sha256 "$TOR_DR_MANIFEST_DIGEST" \
        --restored-pack-root "$TOR_DR_PACK_ROOT"
```

Keep the expected manifest digest in independent custody before restore.
The check requires exactly two PostgreSQL records for the same PACK stem
and exact key, size and SHA-256 agreement with the retained manifest and
the two restored files. Success emits `TOR_DROP_PACK_BACKUP_VERIFIED` and an
object count. A missing receipt or file, wrong digest or size, unexpected
staged file or symbolic link fails the check.

Pair this result with the live reference snapshot and PACK integrity proof
before assessing compaction eligibility. Verify the backup account,
provider-side retention and the recovery point using the
[independent DR procedure](i11-independent-dr-acceptance.md). Keep the CSV
and manifest on the operator host.

## Disposable restore rehearsal

The I-11 `sealed-remote-copy` CI rehearsal includes a synthetic PACK whose
historical FREE V1 is pinned by Saved FREE and Layer after V2 is published.
The encrypted database dump and the PACK, sidecar and retained manifest
are read back from the loopback vault after their working copies are removed.
On the restored database the drill verifies the exact receipts, checks both
historical references and absence of a PostgreSQL body or SINGLE receipt,
then decodes the restored frame. A one-byte change must
be refused by both receipt verification and the PACK reader.

This exercises the restore path with synthetic data. Acceptance of an
independent external account, WAL recovery, provider retention and reader
rights remains part of the independent DR procedure.
