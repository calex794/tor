# DEPLOY-08a — real combined deployment on disposable hosted Ubuntu VM

The independent read-only GitHub workflow uses a fresh GitHub-hosted
Ubuntu 24.04 runner VM, never a self-hosted runner or actual VPS. The lab
provisions actual PostgreSQL 18 package cluster, Nginx, systemd, local
test TLS and an unprivileged tor-deploy identity. Existing default runner
PGDATA is not deleted; only recognized package cluster configurations
are retained separately before creating the dedicated fixture cluster.
No deployment environment, repository secrets, SSH or external S3.
The test uses the default approved Cargo feature profile.

The guarded test calls real DEPLOY-06a preparation and DEPLOY-06b
first-start operations directly inside the disposable VM. The operator
CLI still refuses a GitHub Actions environment. Production main freshness
is not bypassed in any installed helper: the test process injects an
explicit fixture identity allowlist for the synthetic A/B/C/D releases.
A, B and D use the same compiled Rust checkout, with different signed
edge assets and unit restart policy. C is a deliberately signed failing
executable. The lab activation adapter invokes the signed canonical
migration shell (backup, SQLx and privilege checks) and real systemd
restart; the production full go-live shell's remote-main gate and exact
one-click SSH transport are NOT tested by this adapter.

Required actual evidence, emitted only after corresponding assertions:
* DEPLOY08A_REAL_FIRST_START_OK: real initialized PG, signed edge,
  systemd Rust, local HTTPS/readiness, auth/catalog.
* DEPLOY08A_REAL_BACKEND_OUTAGE_OK: stop Rust, preserve HTTPS 502 and
  independent game asset, restart same real binary.
* DEPLOY08A_SIGNED_REAL_UPDATE_OK: signed B, real Nginx generation and
  systemd policy, same retained database book, SQLx and credential hashes.
* DEPLOY08A_REAL_STARTUP_FAILURE_ROLLED_BACK: signed C reaches real app
  cutover, fails, prior B app/edge/unit verified; DB remains unchanged.
* DEPLOY08A_ABRUPT_PROCESS_EXIT_RECOVERED: child actually exits without
  unwinding after live D switch, durable journal blocks next update,
  recover verifies prior B plus data/credential witnesses.

No private data is exported; the only uploaded file is the sanitized
fixture log. The VM is discarded by GitHub; no destructive database
cleanup is needed. This is a real hosted VM test, not an independently
provisioned external DR server, power-off/reboot test, two independent
source SHA upgrade or operational S3 acceptance. The workflow must first
complete successfully; do not treat authored code as acceptance. The
remaining integration with latest main must preserve all Recovery,
security, production build and CI policies.
