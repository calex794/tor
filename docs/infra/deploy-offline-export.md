# DEPLOY-06d — autonomous signed release export

The existing manual main-only "TOR — Обновить сайт" workflow has three
mutually exclusive modes:

* ordinary update: both boolean inputs false;
* prepare_v2=true: sign and transfer without activation to an existing VPS;
* export_v2=true: sign and retain the complete bundle without SSH or SCP,
  even if no previous VPS or deployment host variables exist.

The export still uses the protected tor-staging environment, exact-main
full CI audit, normal approved non-S3 build profile, real isolated TLS edge
acceptance and final main check. The existing signing key is required;
no SSH known-hosts file, old host, or old data access is required.
Signing/deployment key environment variables are scoped to relevant
steps, and temporary private key files are removed at step exit.
No deployment in export mode.

## Artifacts and independent trust

Download tor-offline-v2-<SHA>, containing only the fixed flat verified
signed bundle (all binaries, configs, migration scripts, manifest and
signature). The separate tor-one-click-<SHA> evidence artifact includes
the manifest, signature, public signer, manifest pin and test evidence.
Retain the approved SHA, trusted public signer and manifest digest
independently of the downloaded payload, before any disaster; never trust
a replacement signer or pin merely because it accompanies an archive.
No credentials, database dump, content keys, TLS keys or SSH private key
are exported. Artifacts expire after 14 days; retain approved copies in
independent backup storage if recovery must outlive that interval.

Extract the artifact ZIP as an unprivileged user, not root. Do not execute
files from that extracted directory. Use an independently obtained
reviewed helper checkout and a root-private trusted output parent:

    python3 tools/import_offline_release.py --source /home/tor-deploy/extracted --directory /root/approved-bundle --sha RELEASE_SHA --signers /root/approved-signers --manifest-sha256 PIN

The importer uses the same bounded exact-set copy and signature verification
as the release executor, verifies the independent digest, restores safe
binary/config modes (artifact extraction may lose them), and atomically
publishes a private flat directory only on success. Existing destinations,
tampering, links, extra files, wrong signer and wrong pin are refused.
The workflow exercises the importer before upload. The verified directory
can be passed to DEPLOY-06a preparation, then DEPLOY-06b local start and
DEPLOY-06c admission. For restoring old data use RECOVER instead.
No automatic install, public admission, provider activation or VPS change.

For disaster recovery, retain both independent manifest and signer SHA-256
pins. The evidence artifact includes tor-v2-signers-pin. Use the existing
RECOVERY-01 tool with --format v2 for a non-executable recovery stage,
not the empty-install activation flow. See tor-recovery-release.md.
