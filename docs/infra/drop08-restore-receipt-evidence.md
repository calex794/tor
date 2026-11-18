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

## DROP-08b — historical saved FREE and Layer archive source coverage

An exact S3-manifest-to-PostgreSQL receipt match does not prove that each
**historical entitlement** points to any restored physical source at all.
Before declaring the isolated restore complete, exercise both a saved FREE
edition and a captured historical FREE edition for a purchased Layer after
a newer revision or ordinary book deletion. The required snapshot must contain
both kinds of pins; an absent fixture fails closed rather than being called a
successful rights recovery.

On the **read-only restored PostgreSQL** (not the live site), run:

```bash
set -o pipefail
psql -Xq --no-password --dbname "$TOR_DR_RESTORE_URL" \
  -f tools/drop08_export_historical_free_pins.sql |
  python3 tools/drop08_verify_historical_free_pins.py
```

The SQL exports historical `(kind, chapter_id, version_id)` identities,
verifies that the chapter-version exists and a Layer archive belongs to its
recorded book, then checks for a PostgreSQL inline body, a
registered S349 SINGLE receipt, or an **active** PACK generation with
matching immutable edition SHA-256 and an existing PACK metadata record.
Candidate-only or superseded PACK metadata is not a recoverable reader source. The Python check rejects an unbacked pinned edition, malformed,
duplicate or unsorted exports, a wrong Layer book membership, and a missing
saved/Layer fixture. Results expose
only aggregate counts. The pipe includes private version IDs: never persist it
to public logs or CI artifacts.

**This is metadata coverage, not restored-text proof**: after this check,
the independent DROP-08a manifest/bytes comparison must still pass for every
relevant bucket/prefix, then the actual isolated reader must demonstrate
archived Layer V1 and saved FREE V1, plus revoked access and failure scenarios.
A green unit test or `TOR_DROP08_HISTORICAL_PINS_METADATA_OK` is not an
independent offsite DR acceptance, RPO/RTO measurement or release approval.
The CLI never writes PostgreSQL or S3; no VPS/deploy/migration is performed.
