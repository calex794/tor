# DEPLOY-05 / I-11 — журнал и общий rollback

Stacked on DEPLOY-04. The library is not yet invoked by one-click.
The current root executor still uses its existing production procedure.

## Transaction

The trusted caller holds both the overall release lock and edge lock,
verifies the staged signed bundle and supplies a synchronous application
boundary preserving all existing PostgreSQL checks, backup and migrations.
No service stop, arbitrary subprocess, SSH permission or production
activation is added by this PR.

- Verify old app/edge, prepare signed edge and read-only unit receipt.
- Persist private root-owned journal before every live mutation:
  edge -> unit -> app -> final verify. Atomic write, file and directory fsync.
- Update Nginx first so the fallback remains available during app restart.
- Apply managed systemd policy; then invoke trusted application activation.
- Mark committed only after effective unit, live TLS asset and application
  verification. No automatic schema reversal.
- Any failure: persist rollback_pending, reconcile old edge even if the
  disk link and live workers differ, restore unit policy, restore previous
  application (only restart if app cutover might have run). Verify all
  three before recording rolled_back.
- If any restoration fails retain the journal and refuse a new update.
  Explicit recovery may retry. An interrupted transaction is conservatively
  rolled back, even if all new components happened to become live.
- Journal contains only release SHA, phase, narrow unit policy and effective
  non-secret property receipt; mode 0600 inside 0700. No environment values.
  Terminal record retained until a later release begins.

The v2 verified copy now receives 0755 only after verification, allowing
the non-root web service to traverse it. Copied files and directory
publication are fsynced. Build/sign temporary input stays private.

## Current boundaries

Actual application activation/restoration adapter, legacy checksum
compatibility and trusted helper installation are DEPLOY-05a.
Caller must hold lock until synchronous child exits, including if the
parent is interrupted. There is no boot-time recovery unit yet.
Trace Store active-service handling is still open. No rollback of SQLx
schema or database contents, no automatic pruning, no VPS changes.
Seven architecture tests use the real durable journal and simulate
component failure, abrupt process interruption and retry; not proof
of physical power-loss durability or full live Rust/systemd/PG recovery.

The real Nginx lab now additionally exercises both directions of an
interrupted disk-pointer/live-worker mismatch. Two component regressions
cover receipt drift and edge recovery; stage permissions are asserted.
