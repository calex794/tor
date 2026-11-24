# TOR Recovery / I-11 / S061 — RECOVERY-04b: restore exact offsite object snapshot

Tracking: [#739](https://github.com/calex794/tor/issues/739).
Depends on the accepted RECOVERY-02 workspace and existing DR-06 manifest
and DROP-08 receipt contracts. No live backup, VPS, network or actual
provider acceptance is performed by this change.

## Input and boundaries

On the independently approved clean recovery host, after preparing the
fixed /srv/tor-recovery workspace, keep the following operator-owned 0600
regular files in evidence/:

- objects-manifest.json: independently retained pre-incident DR-06
  manifest for the exact selected archive scope, not a newly generated
  inventory from a possibly depleted source.
- objects-rclone.conf: private recovery-only configuration with exactly
  tor-offsite-read (crypt) and tor-offsite-s3 (S3) aliases. The crypt root
  must be tor-offsite-s3:APPROVED_BACKUP_BUCKET/LITERAL_BACKUP_PREFIX,
  the same encrypted DR-03 archive root that contains objects/ and
  recovery-keys/. Match its original crypt passwords and naming settings.
  The S3 endpoint must be the independently approved HTTPS endpoint,
  env_auth=false, with explicit newly scoped read-only credentials.
  Only documented, supported config options are accepted; never pass
  unrelated inherited remotes, arbitrary backend commands or TLS bypasses.

Verify storage provider/IAM read-only access and independent retention
separately; a different bucket name is not proof of isolation. Never use
the live tore bucket. Independently retain the manifest SHA-256 before
the incident and privately approve the recovery config SHA-256 after
provisioning restore-only credentials. Do not expose either private
configuration or object names in Git, CI, chat or Notion. The fixed
workspace is private; ensure trusted software, network egress policy,
disk quota and capacity. Do not edit the selected manifest/config while
the restore is in progress.

## Validate then download

```bash
sudo python3 tools/tor_recovery_objects.py \
  --source-machine-id "$SOURCE_MACHINE_ID" \
  --target-machine-id "$TARGET_MACHINE_ID" \
  --expected-manifest-sha256 "$APPROVED_MANIFEST_SHA256" \
  --expected-config-sha256 "$APPROVED_CONFIG_SHA256" \
  --endpoint "$APPROVED_BACKUP_ENDPOINT" \
  --archive-root "$APPROVED_BACKUP_BUCKET_AND_PREFIX"
```

By default this checks local prerequisites, config and manifest pins,
exact scope, safe object names and minimum required capacity, without
network or writes. With --restore and
--operator-confirm-read-only-repository it creates a private attempt
marker and a raw file selection from the pinned manifest, then runs a
fixed rclone copy **from** tor-offsite-read:objects **to** the empty
objects/ directory. No remote writes, sync, fallback to source,
arbitrary destination or inherited cloud environment. Download only
the approved keys, not the entire mixed-generation backup bucket.

The supported path grammar is intentionally restricted to ASCII
letters, digits, dot, underscore, hyphen and slash, with no empty,
dot or traversal components or file/directory conflicts. Other
object names require a separately reviewed extension; the tool
refuses instead of silently skipping them. The existing manifest
bounds apply; check disk capacity and quota independently (the
manifest's byte count does not bound malicious oversized responses).

Never rerun into a partial workspace or delete it automatically.
After successful download, recheck the pinned config and use the
existing inventory scanner to compare exact object names, lengths
and SHA-256. A missing object, later unrelated local edition, changed
ciphertext or damaged PACK sidecar fails closed. Successful exit of
rclone alone is insufficient. No raw rclone logs are emitted. Keep
partial evidence private for independent investigation. The attempt
marker prevents an accidental second restore.

## Continue the accepted receipt checks

TOR_RECOVERY_OBJECTS_VERIFIED confirms the staged bytes match the
approved manifest, not full disaster recovery. Reuse
[DROP-08a/08b](drop08-restore-receipt-evidence.md) on the same restored
database and selected scope: compare exact registered FREE SINGLE,
PACK plus manifest and attached paid ciphertext receipts, and verify
saved FREE and Layer historical sources. Verify every other relevant
asset/prefix separately (DROP-08a is not a universal registry of all
bucket contents). Compare all required scopes against independent
pre-incident records. Then prove actual historical paid V1/V2 and
FREE/PACK HTTP reads and ACL/refund denials with recovered keys on the
isolated app, without live payments, writes, or DNS. This step neither
tests decryption nor accepts #739/S061.

Existing architecture tests exercise local files and command doubles,
not the real rclone/S3 service. Operational acceptance of the pinned
config grammar, cloud provider, network, complete inventory and RPO/RTO
is still required.
