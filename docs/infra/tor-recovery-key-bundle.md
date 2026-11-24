# TOR Recovery / I-11 / S061 — RECOVERY-04a: prove application key bundle composition

Tracking: [#739](https://github.com/calex794/tor/issues/739).
Offline operator tool only. Existing DR-03 verifies the ciphertext of a sealed
bundle and its whole plaintext digest, but accepts a nonempty opaque bundle.
That cannot prove a historical application key is included. This slice adds
a strict complementary check, not a new backup system or automatic activation.

## Data to retain independently before a disaster

For a TOR instance with encrypted storage or an encrypted admin S3 profile,
the application-key archive uses JSON schema version 1 with exactly three
top-level fields: schema_version, active_kek_id, keys. keys is an object with
the applicable names below and their canonical encoded 32-byte values:

| Name | Encoding | Requirement |
| --- | --- | --- |
| TOR_CONTENT_KEK_V1 | URL-safe base64 without padding | Required for any content keyring, even V2-only metadata |
| TOR_CONTENT_KEK_V2 | URL-safe base64 without padding | Required if any object refers to V2; retain even after active ID returns to V1 |
| TOR_S3_ADMIN_MASTER_KEY | Standard padded base64 | Required whenever an encrypted admin S3 profile exists |

active_kek_id is content-kek-v1, content-kek-v2, or null only if no content
keyring is present. An absent runtime active setting means V1; record that
effective ID. Keys must be independently generated random 32-byte values;
the tool checks canonical encoding, all-zero rejection and key separation,
not randomness. Never generate replacements for existing keys to make a
check pass. The whole archive and its inventory are private.

This typed app-key bundle is **not** a replacement for the existing full
operator recovery vault. Separately preserve pgBackRest/restic encryption
material, rclone crypt secrets, offline age identity, recovery-only
credentials and relevant configuration history. Do not import old
replaceable cloud/SSH/database credentials blindly after compromise.
Existing opaque DR-03 bundles remain byte-checkable by the old verifier;
they do not automatically pass this new composition gate.

## Obtain key usage from the trusted database

On the approved source before backup and on the independently recovered
database during the drill, export a private JSON usage report (0600,
operator-owned). Use a verified read-only connection with no credentials
in command-line arguments. Include every storage state, not only currently
attached or purchased objects. The query for the current schema is:

```sql
SELECT jsonb_build_object(
    'schema_version', 1,
    'storage_kek_ids', COALESCE(
        (SELECT jsonb_agg(kek_id ORDER BY kek_id)
         FROM (SELECT DISTINCT kek_id FROM public.storage_objects) AS ids),
        '[]'::jsonb),
    'admin_s3_profile_count', (SELECT count(*) FROM public.admin_s3_profiles)
);
```

Unknown IDs, missing fields or invalid counts refuse. An encrypted admin
profile requires its historical master even if inactive. The tool checks the
provided report's coverage; it does not query or authenticate the database.
The operator must tie that report and the chosen key inventory to the
approved backup checkpoint. Do not substitute an empty or fabricated report.

## Capture and verify

On the trusted operator workstation, stream the known-good typed JSON from
the protected vault into this command (the example input file is a private,
temporary vault export, never a repository file). Use a trusted local copy
of the tool and keep the inventory and its separately retained SHA-256 in
independent custody.

```bash
python3 -I tools/tor_recovery_keys.py capture \
  --db-usage /secure/source-usage.json \
  --output /secure/approved-key-inventory.json \
  < /secure/temporary-vault-export.json
```

Capture fails if the current DB references a missing key. The output contains
only key fingerprints and the effective active ID, mode 0600, never overwrites.
Store the SHA-256 of the inventory via a separate protected trust record
before an incident. Seal the same typed JSON using age as in DR-03; do not
persist plaintext on the recovery host. Existing encrypted backup/retention
controls still apply.

On an independent recovery workstation, after restoring and verifying the
selected DB and retrieving the exact sealed key archive:

```bash
set -o pipefail
age -d -i /secure/offline-age-identity /secure/approved-app-keys.age | \
  python3 -I tools/tor_recovery_keys.py verify \
    --db-usage /secure/restored-db-usage.json \
    --inventory /secure/approved-key-inventory.json \
    --expected-inventory-sha256 "$INDEPENDENT_INVENTORY_SHA256"
```

Use the independently recorded pin, not a digest calculated from the archive
during recovery. A valid inventory pin, exact known key set, individual
fingerprints, active ID and DB key coverage are mandatory. A new key of
correct length, missing historical V2, fake rewritten inventory and
unknown future KEK ID all fail closed. No plaintext output or key files
are created by verification. The Python process still holds decoded keys
in memory; use a trusted isolated host and protected RAM/swap/core-dump
policy. Python does not provide guaranteed secret zeroization.

## Remaining gates

A successful TOR_RECOVERY_KEYS_VERIFIED means the supplied typed bundle
matches the pre-incident inventory and covers the supplied database report.
It does **not** prove a key decrypts a real object/profile, the report is
authentic, every object exists, or a safe operational restore. Test actual
historical paid V1/V2 and admin profile decryption and complete object
inventory, negative ACL checks, independently protected offsite storage
and measured RPO/RTO under the existing I-11 acceptance procedure.

No existing backup timers, restore commands, cloud, VPS, secrets or runtime
configuration are changed. The operator must adopt the typed bundle after
review; no claim that real backups have been migrated or verified.
