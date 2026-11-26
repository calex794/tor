# DEPLOY-05b — переход одной кнопки на v2

Same manual main-only GitHub Actions workflow, existing protected
environment and exact-main CI gate. A root-owned readable marker
/var/lib/tor-release/release-format with content 2 selects the signed
v2 protocol. Missing marker selects legacy v1 during migration only;
set environment/repository variable TOR_REQUIRE_RELEASE_V2=1 to fail
closed if the marker is absent. Invalid marker/SSH output refuses release.

New prepare_v2 input is false by default. True signs and transfers v2
to /opt/tor/incoming/v2/<sha>, but never invokes the sudo executor,
performs migrations or checks a newly installed release. It is for
one-time operator bootstrap. The v2 build includes the four S3 Cargo
features (not test-cdn-loopback); compiled support alone does not enable
runtime flags or prove external provider acceptance. Legacy v1 build
remains unchanged here. The already configured S3 runtime must not be
served with an incompatible build.

Both release modes preserve the original current-main and CI gates.
V2 signs every file using the distinct namespace, verifies before
transfer and again in the root executor. The same sudo path accepts only
one full SHA. Its installed v2 wrapper refuses incomplete bootstrap or
legacy inbox; it never dispatches to v1. SSH recovery remains root-only.
The exact v2 signature/manifest are uploaded as evidence (no keys).

The wrapper and root marker are not installed automatically by Actions.
The operator bootstrap must install trusted helper modules, verify the
staged bundle and adopt Nginx before publishing the marker. The root-only bootstrap in this PR performs these operations. Bootstrap copies v2 into a separate root-private staging area. It never
replaces the existing Rust release directory. The first actual v2 update
must use a fresh SHA if the current app directory is still v1.
No VPS update is triggered by this PR.

## One-time operator procedure (after acceptance and merge)

1. Run the existing button on main with prepare_v2=true. No activation.
2. Root inspects the real vhost and independently approves its SHA-256.
3. Root obtains the reviewed bootstrap script from the exact approved main
   commit and runs it with APPROVED_MAIN_SHA, canonical VHOST and approved
   VHOST_SHA256. It fetches only fixed helper files from that immutable SHA.
4. It holds the existing lock, verifies the signed staged bundle, validates
   the current Nginx configuration, installs private helpers, saves the old
   fixed executor, and replaces the sudo target with the fail-closed v2
   wrapper. It adopts Nginx without stopping Rust or touching PostgreSQL.
5. Only after verified HTTPS edge acceptance and rechecking main is the
   public root-owned marker written. A retry before publication must use
   identical approved helper bytes; unknown partial adoption is refused.
6. Set TOR_REQUIRE_RELEASE_V2=1; subsequent ordinary button runs use v2.

A crash before marker publication blocks normal sudo updates, not the
running website. Root may retry the approved bootstrap; no automatic v1
fallback. A fully installed helper upgrade is a separate operator procedure.
Existing sudoers and secrets are preserved. Real systemd/Rust/PG acceptance
and an exact-main edge acceptance gate are still required before rollout.
