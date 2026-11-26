# TOR Recovery / I-11 / S061 — autonomous release recovery (RECOVERY-01)

Status: offline release verification/staging only, no production change or
operational DR acceptance. Tracking: [#739](https://github.com/calex794/tor/issues/739).
Use the [independent DR runbook](i11-independent-dr-acceptance.md) for data
recovery, rights verification and measured RPO/RTO.

## Why a separate path is necessary

The existing "Update site" workflow requires GitHub current main, a healthy
TOR, active PostgreSQL and a previous installed binary. It keeps checksum
artifacts for 14 days, not an independent archive of executables. Neither
this workflow nor a VPS snapshot proves recovery from loss of the VPS or
GitHub account takeover. Do not run ordinary deployment against a blank host.

## Prepare before an incident

On an independent trusted workstation keep a verified copy of the recovery
tool, its documentation, repository/source history and reproducible host
configuration. Preserve at least the last known-good releases, their exact
Cargo feature profile, compatible schema and the actual eight release files:

- tor-web, tor-migrate, tor-trace-store
- their three .sha256 files
- tor-release-manifest and tor-release-manifest.sig

The eight files are already produced by the existing signed release procedure.
Copy them to independent durable storage before the incident, after reviewing
a successful release and post-merge CI. Do not use a publicly accessible
archive. The code/signature trust root and manifest SHA-256 must also be
independently retained, outside the production VPS and the GitHub account.
In particular the existing deployment SSH key signs the manifest; possession
of that key can sign malicious binaries. Pinning the independently approved
manifest digest prevents accepting a newly signed replacement solely on that
basis. Record the trusted allowed_signers file and its SHA-256 separately.
A pin derived from the archive after the incident is not independent proof.

Record the exact accepted release SHA, platform/architecture, Cargo features,
schema compatibility and backup checkpoint references in the protected
operator vault. Preserve PostgreSQL full/diff + WAL, all object bytes and
the offline encrypted recovery-key bundle separately. In particular retain
TOR_S3_ADMIN_MASTER_KEY and all historical content KEKs, as well as the
backup decryption material and recovery-only credentials; do not put private
keys or production backups in GitHub artifacts. See
[i11-s3-keys-offsite.md](i11-s3-keys-offsite.md).

## Offline recovery on a clean host

The following placeholders are values from the independent operator vault,
not automatically discovered from a compromised VPS or current main:

```bash
python3 tools/tor_recovery_release.py \
  --bundle /secure/recovery/release-files \
  --sha "$APPROVED_RELEASE_SHA" \
  --manifest-sha256 "$APPROVED_MANIFEST_SHA256" \
  --allowed-signers /secure/trust/tor-release-allowed-signers \
  --signers-sha256 "$APPROVED_SIGNERS_SHA256" \
  --stage-root /secure/recovery/staging
```

Use a pre-existing private, operator-owned staging root. The command checks
the exact release manifest, independent SHA-256 pins, real SSH signature,
all three binary SHA-256 hashes and the exact eight-file inventory. No
symlinks, FIFO, unexpected files, existing stage overwrite or binary
execution. With staging, verified bytes are copied into a new private
tor-recovery-SHA directory with non-executable mode 0600. Omit --stage-root
for read-only validation. Re-run verification on the staged directory
before any later installation; interrupted/failed operations cannot
authorize activation. Work with a trusted, quiescent local copy, not a
live attacker-writable directory.

A successful TOR_RECOVERY_RELEASE_STAGED does NOT mean a usable TOR,
compatible Linux executable, correct Cargo features, recovered PostgreSQL,
backup freshness, independent trust custody or safe activation. It proves
only the selected offline release bytes match the supplied pins and signature.
Actual offsite release retention is a separate operator task.

## Next stages — no auto-deployment

1. Prepare independent recovery media and a clean-host bootstrap with fresh
   credentials, local-only listeners and no live payments/mail/S3 writes.
2. Recover exact PostgreSQL backup + WAL to a selected clean point, and
   full S3 inventory plus historical keys from protected external archives;
   never silently fall back to the primary or a newer possibly poisoned copy.
3. Validate schema, recovered configuration, object hashes and paid/free
   historical rights on the clean isolated host. Use the existing evidence
   gate and measure RPO/RTO (targets 15 minutes / 4 hours, not guaranteed).
4. Only after independent verification and explicit operator approval,
   rotate compromised credentials, reconcile payment events, fence the old
   instance, then allow traffic/DNS cutover. No automatic public activation.

No SSH, VPS access, backup writes, cloud changes, service installation,
migrations or new CI workflows in RECOVERY-01.

## DEPLOY-07a — signed v2 recovery releases

The same RECOVERY-01 verifier accepts the explicitly selected --format v2.
The default remains v1; no format guessing or fallback is permitted.
V2 uses the full exact allowlist of the accepted release_bundle_v2 tool,
including signed Nginx, fallback, unit, install and migration files. Keep
that specific independently reviewed tool version and its import helper
with the archive; the allowlist can change between future releases.
The signature namespace is tor-release-v2, distinct from legacy v1.

Use the same CLI and independent SHA, manifest SHA-256 and signer SHA-256,
with --format v2 and optionally --stage-root /srv/tor-recovery/release.
The v2 verifier takes a private snapshot of the independently pinned
signer, reuses the bounded signed import, checks the manifest pin, and
publishes only the exact verified bytes at tor-recovery-SHA, all mode 0600.
Without --stage-root it uses an ephemeral private complete copy; ensure
sufficient scratch disk. No current-main dependency, SSH, service start,
SQL, production backup or DNS. Do not run the clean empty installer on a
recovery workspace. Use the existing physical PG/WAL, object and key
recovery steps and independent rights/ACL acceptance, with an isolated
application boot to be implemented separately. Recovery still needs the
accepted S3 feature profile for the selected data; a signed default build
is not evidence that a recovered S3-configured TOR can read paid content.

The export_v2 artifact evidence now also records the public signer digest.
Retain independent trust and approved checkpoint metadata BEFORE loss,
not only evidence alongside the GitHub artifact. This step only verifies
and stages release bytes, not data recovery or production readiness.
