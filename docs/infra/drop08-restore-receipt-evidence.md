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
verifies that the chapter-version exists, the chapter is FREE with zero
price, and a Layer archive belongs to its recorded book, then checks for a
PostgreSQL inline body, a
registered S349 SINGLE receipt with matching immutable edition SHA-256,
or an **active** PACK generation with matching immutable edition SHA-256 and an
existing PACK metadata record.
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

### Disposable PostgreSQL acceptance fixtures

The mandatory `postgres-runtime (foundation)` job also executes
`bash tools/drop08_pg_fixture_ci.sh` on its temporary PostgreSQL 18 instance,
after SQLx migrations. The script refuses every database URL except the fixed
loopback `tor_ci` migrator fixture and creates **temporary session-local
tables only**. It drives the **same** export SQL and verifier used in the DR
procedure, covering:

- Both saved SINGLE V1 and historical Layer V1 in an active PACK, accepted.
- Layer archive associated with the wrong book, refused.
- A non-FREE chapter incorrectly pinned as a Layer FREE archive, refused.
- S349 SINGLE with a different immutable edition SHA-256, refused.
- PACK generation with a different edition SHA-256, refused.
- PACK candidate not activated for the reader, refused.

This fixture verifies query shape and source-classification logic on real
PostgreSQL syntax. It does not model live FK constraints, decryption, S3
availability, reader authorization, or independence of backups. Those remain
mandatory operator acceptance gates.

## D-001 — selected obsolete FREE SINGLE restore evidence

Reuse the existing encrypted append-only offsite object archive. Do not create
a second backup system, use the current primary as a fallback, or infer an
old edition from its text hash. On the independent recovery host select one
exact immutable receipt from the recovered PG snapshot (source bucket,
chapter ID, version ID). Retrieve that exact object into a dedicated empty
private directory under the full original object key using the approved
read-only offsite configuration. Retain the independently pinned, original
manifest and its SHA-256, not a new manifest generated from the retrieved
file. The manifest may cover other objects; the selected staging tree must
contain exactly one file.

```bash
set -o pipefail
psql -Xq --no-password --dbname "$TOR_DR_RESTORE_URL" \
    -v bucket="$TOR_DR_SOURCE_BUCKET" -v chapter_id="$TOR_DR_CHAPTER_ID" \
    -v version_id="$TOR_DR_VERSION_ID" \
    -f tools/d001_restore_single_receipt.sql |
    python3 tools/drop_verify_restore_receipts.py \
        --manifest "$TOR_DR_PRIVATE_MANIFEST" \
        --expected-manifest-sha256 "$TOR_DR_MANIFEST_DIGEST" \
        --restored-single-root "$TOR_DR_SELECTED_RESTORE_ROOT"
```

TOR_D001_SINGLE_BACKUP_VERIFIED proves exact immutable receipt/manifest/file
agreement. It is neither a removal authorization nor a policy for reopening
a closed edition. The existing I-11 disposable lane archives a synthetic old
SINGLE in its separate loopback vault, discards the local source, verifies a
vault readback, and only then invokes the privileged D-001 fixture seal. The
subsequent encrypted PostgreSQL dump must preserve the exact seal timestamp,
exclude the edition from the fresh candidate queue, deny a fresh delivery
hold, and preserve the current synthetic V2. The recovery test downloads the
historical file again, checks the exact restored SQL receipt, rejects corrupted
bytes and verifies a fresh download. This is not production provider,
credentials, administrative history, or independent physical host acceptance.
No primary S3 DELETE, restore-to-live, reopening or automatic activation is added.
