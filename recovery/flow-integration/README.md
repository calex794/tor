# FLOW integration recovery checkpoint

Captured 2026-11-11 14:11 UTC. **The FLOW line is incomplete.** This directory records integration knowledge after the execution environment became unavailable. It changes no application source and is not an acceptance PR.

## What is durable

Published PRs #819, #822, #825, and #826 contain their complete source. #819 is merged with 17/17 successful post-merge jobs. #822 is merged with successful exact-head CI; its post-merge run was still running at capture. #825 and #826 remain drafts; their separate REC acceptance and repository rules succeeded. Their full Integration CI is required before merge.

Browser jobs in #825 failed during APT font/dependency installation before browser tests: DNS resolution failed for the runner's Ubuntu mirrors. The runner already contains the official Azure/archive/security mirror fallback. Retry failed jobs on the same source revision after the whole run completes; do not remove dependencies, skip tests, or treat REC as a substitute.

Two separate recovery branches preserve authoring source and edit recipes:

- [H HTML health](https://github.com/calex794/tor/tree/wip/flow-html-recovery-20261111/recovery/flow-html)
- [FLOW09 genres and public pages](https://github.com/calex794/tor/tree/wip/flow09-recovery-20261111/recovery/flow09)

Their first checkpoint commits and exact inventory are in snapshot.json. Owners may append explicitly unverified fixes later. These are authoring records, not a compiled tree or a byte-equivalent snapshot of the unavailable VM.

## Source that is still missing remotely

The complete signals/quality base `dd8afae` + `c5c5cc5` + `cc418415a813c527237a54b71426c9a4c69abf9b` exists only in the unavailable local Git repositories. Do not infer that saving the recovery branches saved this base. Restore the existing environment and immediately publish a WIP branch for the intact local commits before editing. Do not fabricate missing source from descriptions.

The root integration worktree is `/workspace/scratch/1e86e471c018/tor-final`, branch `feat/flow-final-integration`, last observed head prefix `bc72b1eb8`. The last full hash was not read. Local commit prefixes and migration reservations are in snapshot.json.

## Required restoration sequence

1. Preserve the intact local signals and integration commits remotely. Inspect dirty working trees before applying authoring recipes; several recipes were already applied locally before the outage.
2. Reconcile the H and FLOW09 recovery directories against their actual local files. Preserve the existing published exposure/editorial fixes.
3. Resolve H's confirmed false-completeness cases: a non-retired expired worker must keep later completeness unknown; startup losses must not disappear into a baseline; missing optional health schema must preserve available legacy counts with unknown quality; timestamp bounds must account for both ends of database round-trip time.
4. Inspect the actual F health SQL for analogous expired-worker/startup-loss errors. Those F defects are a required investigation, not a confirmed code finding while the source is unavailable.
5. Finish genres over the full distinct genre set before result LIMIT, and the explicitly bounded public-page sample. Unknown first failures, network failures, origin/revision changes, stale leases and lost inventory must not be converted into numeric zero or a page-loss alarm.
6. Finish navigation and documentation on the revised model, update route inventory without renumbering existing routes, and verify migrations from the installed schema and on a fresh database, then replay.
7. Run focused PostgreSQL, HTTP and real browser regression on the final combined revision. Publish reviewable implementation PRs, run full Integration CI and relevant acceptance, merge in dependency order, then verify post-merge CI.
8. Reconcile the existing FLOW and Fast Notion pages with evidence. Do not increment their formal completion counters for partial packages.

## Local evidence and its boundary

Before the outage, the combined 201-migration fresh database and replay, canonical/SEO/signals PostgreSQL and HTTP tests, and actual-server audience/exposure/editorial browser acceptance passed. The browser captured 390 and 1440 widths; the signals dashboard then had 11 rules. These results precede the unfinished H health and FLOW09 genre/public-page work, and precede the last bootstrap-off integration change.

The published editorial tree passed the complete 415-test architecture suite. The final integration architecture run's result is unknown because the environment disappeared while it ran. Do not transfer any earlier PASS to a new reconstructed revision.

The bootstrap-off change `d68f97e` / root `b108688ba` preserves secure report history while the issuer/worker is disabled or startup readiness fails; final HEAD must retest it. It is not in #825 or #826.

Relevant scratch logs/scripts (if disk returns): `flow-final-fresh-integrated.log`, `flow-final-architecture.log`, `editorial-architecture-final-fixed.log`, `run-flow-final-integrated.sh`, `run-editorial-integrated.sh`. Use a fresh isolated PostgreSQL directory for combined acceptance. A prior failed exposure assertion came from reused browser fixture IDs and leftover aggregates; the fixture cleanup was fixed and the fresh rerun passed. Do not weaken the assertion.

## Product and operational boundaries

No individual REC/FLOW behavioral collection or consent UI is restored. Keep canonical business facts and P1 Hide. Anonymous observations mean a card met the geometry/time/foreground rule, not a person, billed impression, or guaranteed attention. First confirmed chapter completion per account/book is not full-book completion. Missing observations or health coverage stay unknown.

Editorial promotion covers owned placements, not paid S042/S043 advertising or money. Live Google/Yandex access is not connected and is not accepted by fixture/wire tests. Public-page monitoring covers a bounded sample, not the whole catalogue, indexing or independent outside reachability.

No production deployment, VPS access or production database mutation is authorized by this checkpoint. Ordinary reviewed merges after full gates remain within the existing task authorization.
