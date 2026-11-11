# I-11 / DR-03: offsite S3 objects and sealed recovery keys

Status: operator-only templates and architecture tests, NOT installed, scheduled, or proved on a real external provider.

## Manual backup contract

- ops/backup-s3-offsite.sh requires root and a root-only 0600 config /etc/tor/offsite-objects.env. It refuses GitHub Actions.
- Configure TOR_DR_PRIMARY_REMOTE (rclone alias tor-primary-ro, READ only); TOR_DR_OFFSITE_REMOTE (alias tor-offsite-crypt, separate account/provider); TOR_DR_RCLONE_CONFIG; TOR_DR_AGE_RECIPIENT; TOR_DR_SECRET_BUNDLE (root-only file containing all active and historical KEKs and necessary recovery secrets).
- The target must be rclone crypt backed by an independently protected S3 bucket with versioning/Object Lock and distinct credentials. Separate aliases do not themselves prove distinct IAM or provider.
- Copy (not sync): --immutable never overwrites existing objects and does not propagate deletions. Byte-level --download --one-way check decrypts the copy and compares it to the source. This can be expensive; review traffic and quotas before running.
- The recovery bundle is encrypted using the age PUBLIC recipient key, and only the sealed ciphertext is stored offsite; keep the PRIVATE age identity offline on a different trusted machine. Save the returned recovery-keys selector privately.

## Independent restore acceptance

- On another trusted machine set /etc/tor/offsite-restore-objects.env, root-only 0600. Required: TOR_DR_RCLONE_CONFIG, TOR_DR_OFFSITE_READ_REMOTE (alias tor-offsite-read, read-only), TOR_DR_AGE_IDENTITY (offline private key file), TOR_DR_KEY_ARCHIVE, TOR_DR_EXPECTED_OBJECT, TOR_DR_EXPECTED_OBJECT_SHA256, TOR_DR_EXPECTED_SECRET_SHA256.
- The expected private ciphertext SHA-256 must be taken from a separately restored and verified PostgreSQL storage_objects registry; the key-bundle SHA-256 comes from an independently preserved operator record. Never put keys or private identities in Git, Notion, CI or HTTP logs.
- ops/verify-s3-offsite-restore.sh retrieves one private V1 ciphertext, verifies it, decrypts the sealed recovery bundle with the offline age identity and checks the independent secret checksum. Files are deleted from an isolated temporary directory after the check. It does not write to live S3/PG or start TOR.

## Outstanding acceptance conditions

- Physically verify offsite separation, IAM denies deletion, bucket retention and the complete asset inventory. Perform the full PostgreSQL/WAL + S3 + KEK + purchase/refund/ACL restoration on a clean independent host, record elapsed recovery and measured RPO/RTO.
- Existing tools/tor_s334_paid_restore_smoke.sh proves private V1 recovery only against disposable CI PostgreSQL and MinIO with a runner-local KEK, not actual independent key custody.
- No deployment, production migrations, release approval, automatic cron/timer or activation is authorized by this change. Issue #739 and TOR-HOME-01 retain operational blockers.

References: docs/infra/i11-offsite-backup.md, docs/infra/i11-pgbackrest-wal-pitr.md.