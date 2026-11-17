# DROP-08a — check object backup coverage against recovered PostgreSQL

The independent disaster-recovery drill needs three kinds of evidence at the
same recovery point: recovered PostgreSQL state, restored object bytes, and
independently held content keys. The existing [I-11 runbook](i11-independent-dr-acceptance.md)
covers the recovery environment, WAL, isolated keys and RPO/RTO. This check
closes the object-receipt comparison step for FREE SINGLE, both PACK objects
and attached paid chapter/asset ciphertext.

On the isolated restore host, use a SELECT-only role and the frozen database
chosen for the drill. Select the source bucket and literal key prefix that
the retained pre-incident manifest represents. The staged object tree must
retain the full object keys. First use the [I-11 manifest verifier](i11-offline-inventory-evidence.md)
to confirm bytes, lengths and SHA-256 against the independently retained
manifest and digest. Then compare that same manifest with PostgreSQL.

```bash
set -o pipefail
psql -Xq --no-password --dbname "$TOR_DR_RESTORE_URL" \
    -v bucket="$TOR_DR_SOURCE_BUCKET" -v prefix="$TOR_DR_KEY_PREFIX" \
    -f tools/drop_restore_receipts.sql |
    python3 tools/drop_verify_restore_receipts.py \
        --manifest "$TOR_DR_PRIVATE_MANIFEST" \
        --expected-manifest-sha256 "$TOR_DR_MANIFEST_DIGEST"
```

The SQL reads one consistent snapshot and never changes it. The verifier
requires exact equality of names, sizes and SHA-256 and reports only aggregate
counts. A missing receipt, missing manifest sidecar, unexpected key, wrong
checksum, truncated export, duplicate or unsorted row fails the comparison.
If a prefix contains more than one million objects, divide it into disjoint
literal prefixes and account for every prefix in the backup set. Keep the
manifests and logs with raw keys off GitHub and outside public tooling.

Record the selected recovery point, receipt verification result, actual
application reads for saved FREE V1 and purchased V1 after V2, denial cases,
key access, measured RPO/RTO and the operator's evidence of independent
storage. The comparison is one component of DROP-08. It cannot establish
provider/account independence or recover rights from a PACK manifest.
