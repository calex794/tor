# DROP-04e — worker failure matrix and measured client RSS

Original plan: DROP-04, DROP-B03/B05/B06/B07/B08. Builds on accepted #1005.
This slice adds tests and CI measurements only. No application behavior,
production flags, deployed migrations, VPS or external cloud changes.

## Real storage fault relay

The CI-only relay accepts only the destination PACK namespace in the existing
loopback MinIO fixture. It preserves signed Host/path/query/payload and forwards
actual requests to real MinIO. It never stores or prints bearer headers or text.
Each injected fault must be observed by a hit counter, not inferred from timing.

| Transition / fault | Required worker result |
| --- | --- |
| Destination PUT unavailable | retry, no stage/publication, keep source; recover using the same identity with rollout OFF |
| PACK PUT committed, ACK lost | full independent readback proves bytes; complete exactly once |
| Manifest PUT committed, ACK lost | full independent readback proves both objects; complete exactly once |
| First PACK GET unavailable | no stage/publication, retain source; safe retry |
| Finalizer reproof GET unavailable after durable stage | keep stage, no publication, retain source; safe retry |
| Corrupted PACK bytes over the wire | no publication; healthy retry of immutable destination |
| Lease stolen after real GET | stale worker cannot stage/finalize/delete source; reclaimed epoch completes |
| Hold set after staged proof and second GET | fail closed before publication, stage remains inert |
| PostgreSQL stage INSERT fails | transaction rolls back, no live pointers; retry succeeds |
| PostgreSQL second chapter publication fails | rollback the *whole batch*, including first publication and active placements; retry succeeds |
| Source cleanup unavailable after COMMIT | succeeded state and exact PACK remain; next worker is idle; source orphan awaits reconciliation |

SQL failures are injected on the actual PostgreSQL tables with scoped CI-only
triggers raising SQLSTATE 08006. A sequence counter survives rollback and proves
the injected branch really ran. The app role is unchanged; only the isolated CI
admin can install/remove the trigger. Each trigger is removed before outcome
assertions and the next worker retry.

Every recovery checks exact edition/publication counts, no fake SINGLE and
unchanged immutable transport. Storage failures leave no partial public batch.
The original real-PG/direct-finalizer test already covers lost COMMIT ACK replay;
this slice does not claim an actual severed PostgreSQL TCP COMMIT response.

## Resource evidence

The existing 100000-chapter exercise remains 200 bounded jobs of 500. Linux
`/proc/self/status` now reports baseline client RSS, process peak RSS and peak
growth. These include runtime/client overhead; they do not measure PostgreSQL,
MinIO, a cloud bill, real-book compression or total VPS memory. Input/frame caps
remain the accepted S023 and G limits; no workload limit is weakened.

## Acceptance

- Existing real PG18/MinIO/Chromium and durable transport profile remains required.
- New `drop_04e_worker_faults` runs after that profile in fresh disposable `tor_test`.
- Required exact-head CI, Repository rules and post-merge evidence before acceptance.
- Full original plan still includes active Single→PACK switching, inventory,
  retention-aware GC/compaction, independent restore and real-provider acceptance.
