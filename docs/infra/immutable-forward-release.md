# Immutable forward-only one-click release

The manual main workflow pins the dispatch SHA. A fast-forward of main during
CI, packaging, transfer, SQLx or activation must not invalidate that release.

## Contract

* Both required **main push** workflows and every required job/matrix child
  must be successful for the exact pinned SHA. The live protected branch
  policy and main ancestry are checked before building and again after
  transfer. No deployment from PR runs or an off-main SHA.
* The root-installed ancestry verifier requires the active release SHA to
  equal or be an ancestor of the candidate, and the candidate to equal or
  be an ancestor of a freshly resolved main SHA. The comparison uses full
  immutable SHAs and verifies the merge base and direction. No timestamp
  ordering, force update, branch-name trust or fallback on API failure.
* The same VPS release lock encloses the ancestry check and installation.
  The trusted root helper verifies the existing signed manifest and all
  checksums before running the pinned activation script. The activation
  script rechecks forward ancestry before SQLx and before cutover.
* Backup, SQLx validation, production S3 profile, root-only recovery approval,
  HTTPS readiness and verified rollback to the prior binary remain intact.
  Ordinary manual go-live and standalone evidence audit retain their
  strict exact-main behavior. The forward mode is explicit and root-only.
* The deploy workflow is still manual-only, single-flight and main-only.
  Stage-only mode still performs no activation.

## One-time upgrade (operator as root)

After merge and exact-SHA CI acceptance, select a full reviewed main SHA
containing this change. Download the bootstrap from that immutable commit,
check its contents, then run it as root with that same SHA. The bootstrap
installs the root-owned helper and ancestry verifier, checks current
production readiness, and does not apply SQLx or restart the service. It
accepts an approved main ancestor so parallel PR merges need not stop.

```sh
sha=<full-reviewed-main-sha>
curl -fsSL "https://raw.githubusercontent.com/calex794/tor/$sha/ops/bootstrap-one-click-release-vps.sh" -o /root/tor-bootstrap-forward.sh
bash -n /root/tor-bootstrap-forward.sh
bash /root/tor-bootstrap-forward.sh "$sha"
```

Require `TOR_ONE_CLICK_BOOTSTRAPPED` and `TOR_RELEASE_ANCESTRY_OK`.
Do not run the bootstrap during a release. Subsequent releases use the
same GitHub Actions button with stage_only off. The SSH deploy user
cannot modify the verifier or request a backward release. Public GitHub
ancestry API outages/rate limits remain fail-closed; retry after recovery.

## Acceptance

Test equal and advanced main, old-candidate replay, diverged history, malformed
API evidence, missing CI and non-main runs, failed network, unchanged signature
and recovery contracts. Test the real updated root helper on the VPS with an
ordinary release before claiming production rollout complete.
