# DEPLOY-05b — переход одной кнопки на v2

Same manual main-only GitHub Actions workflow, existing protected
environment and pinned-main exact-SHA CI gate. A root-owned readable marker
/var/lib/tor-release/release-format with content 2 selects the signed
v2 protocol. Missing marker selects legacy v1 during migration only;
set environment/repository variable TOR_REQUIRE_RELEASE_V2=1 to fail
closed if the marker is absent. Invalid marker/SSH output refuses release.

New prepare_v2 input is false by default. True signs and transfers v2
to /opt/tor/incoming/v2/<sha>, but never invokes the sudo executor,
performs migrations or checks a newly installed release. It is for
one-time operator bootstrap. Both protocols use the exact reviewed production capability profile from
tools/build_tor_production_web.sh, accepted by main PR #1261. Compiled S3
capabilities do not authorize a provider, data migration or runtime flags.
Existing stage_only remains available for the selected installed protocol.
The explicit prepare_v2, export_v2 and stage_only inputs are mutually exclusive.

Both protocols use the shared forward-only Git ancestry and exact-SHA CI gates.
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
is still required before rollout. The v2 workflow now runs the real TLS
edge lab from the selected checkout before signing and transfer; its success
is required and its log is retained. A pending wrapper without marker blocks
ordinary releases; a v2 marker without the matching wrapper also refuses.
A v2 invocation must report the expected exact-SHA v2 success marker.

Ordinary updates in either protocol prove candidate ancestry on current main
and from the active release using the shared #1292 release_ancestry module.
V2 installs its own private root-owned copy of this module; the signed
activation shell rechecks ancestry before migrations and before cutover.
Manual clean activation, admission and bootstrap still require exact current
main. Offline historical recovery verification has no live main dependency.
