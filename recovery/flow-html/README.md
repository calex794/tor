# Incomplete FLOW HTML health recovery checkpoint

This directory preserves unfinished authoring work after the shared execution
environment became unavailable on 11 November 2026. It is **not an implementation
PR, a release, a passing CI revision, or permission to merge/deploy**.

The branch starts from published repository commit
`85f47ab9dbfa1ec15562e6e0602c61d08a9582fd` only to make this backup durable.
The enclosed edits require a different, currently unavailable local base:
`cc418415a813c527237a54b71426c9a4c69abf9b` on `feat/flow-signals`.
That base includes initial signals commits `dd8afae` and `c5c5cc5d` and
the separate exposure health integration. It is not recoverable in full from
this checkpoint. Do not apply these edits directly to the published branch base.

## Inventory and provenance

- `sources/` mirrors the intended repository paths for the new H/RIL SQL,
  domain, repository, service, tests and contract files.
- `existing-file-edits.json` records ordered text substitutions, an optional
  replace-between operation, and append/insert operations against the local base.
- `input-regression-edit.json`, `http-browser-edits.json` and
  `document-edits.json` preserve additional authored edits.
- `checkpoint.json` records the known local validation and outstanding work.

Contents were reconstructed from retained authoring tool inputs. They are not
guaranteed byte-identical to the final local Rustfmt output. Formatting and
comments may differ. The old local PASS results must not be attributed to a
newly reconstructed tree without running that tree.

## Blocking findings still to fix

1. The current health read ends an epoch at `lease_until_ms`. A non-retired
   worker which lost database access may keep serving HTML with collection
   disabled. Another healthy worker can then certify later hours. Such an
   unresolved worker must keep later coverage unknown until its end is proved;
   a later retire must not backdate its stop to the old lease expiry.
2. Starting an epoch with current loss counters as its baseline can forget
   known rejected samples during startup or a registration gap. Preserve the
   interval and known loss evidence, including a gap crossing an hour boundary.
3. `observed_health_report` currently fails the whole observation read when the
   health schema is absent. A legacy capability fallback must preserve available
   historical counts and explicitly unknown quality. A failure reading an
   existing health schema must still be an error, not a healthy fallback.
4. A further clock-boundary question is not yet verified: the monotonic sample
   is captured before awaited SQL, so the nominal positive clock tolerance may
   expand by SQL round-trip time. Check that a known queue loss near a UTC hour
   boundary cannot fall outside the durable loss flags. Use a deterministic
   boundary test and account for the full clock uncertainty.
5. Finish the independent review and add focused PostgreSQL regressions for
   these cases. Also inspect the analogous exposure ledger assumptions.
6. Run the final owner PostgreSQL/HTTP/browser suite, inspect fresh 390/1440
   screenshots, then commit and integrate through the normal exact-revision CI.

## Validation which completed before the environment failure

On the previous local working tree: additive migrations 200/200 and replay,
RIL/HTML PostgreSQL 10/10 (three new cases), traffic behavior units 51/51,
HTML/reading adapter units 2/2, existing audience HTTP 1/1, all-target Clippy
for traffic behavior/web, formatting and feature layout passed.
Additional health-table HTTP lock assertions compiled under Clippy; their final
runtime repeat and the new durable-health browser screenshots did not finish.

Passing these earlier cases did not settle the later review findings above.
The base signals/exposure-health local tests also do not make their unpublished
commits remotely accepted. No production database, deployment or main ref is
changed by this checkpoint.

## Recovery sequence

Recover the required local base from the workspace Git objects if possible.
Compare these authoring sources with the surviving uncommitted files before
writing anything. Apply ordered edit operations only when their original text
matches; fail on a missing anchor. Keep new H files under their original paths,
run the pinned formatter, resolve the blocking findings, and validate the new
tree as a new revision. Root integration owns shared worker/render conflicts,
the complete 13-rule signal set, startup/shutdown wiring and final acceptance.
