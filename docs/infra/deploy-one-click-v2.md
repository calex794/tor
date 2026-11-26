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
staged bundle and adopt Nginx before publishing the marker. The next
step adds/tests that bootstrap. A first preparation must use a SHA not
already installed as a v1 release (no overwriting live directories).
No VPS update is triggered by this PR.
