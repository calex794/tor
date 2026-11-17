# DROP-09 — staging and rollback

TOR starts with an empty catalogue. Single interactive publication,
PACK-direct queued imports, and incremental Single→PACK cutover have
different rollback properties. This is a release procedure for a future
approved rollout.

## Evidence before any cutoff

Record the exact accepted SHA, rollout window, provider/storage class,
separate restore point, object-backup manifest, full CI and
[provider measurements](drop09-yandex-provider-acceptance.md). Open a
separate temporary deployment on disposable PostgreSQL and S3 with the
same build flags and credentials restricted to the staging resources.
Apply only that SHA's migrations using the migrator role. Keep service
and migrator credentials separate.

Execute the independent restore procedure with WAL, object archives and
key recovery. Include explicit checks of saved FREE V1, purchased V1,
new V2, actual S3 Range, revoked access and wrong or absent backup
objects. Use the measured RPO/RTO and named owners for the release
decision. A synthetic MinIO run is supplementary evidence.

## Observe one change at a time

| Step | Change | Acceptance before advancing |
| --- | --- | --- |
| Baseline | All DROP flags at 0 | Authenticated runtime readiness, SINGLE publish/read/edit, paid V1, private buckets and backup health |
| Reader | `TOR_FREE_PACK_READER_ENABLED=1` | Exact transport and version selection, anonymous/hold/deletion denials, saved V1 after V2, desktop/mobile and no-JavaScript profiles, bounded latency |
| Build candidates | `TOR_DROP_PACK_WORKER_ENABLED=1`, then `TOR_DROP_PACK_PLANNER_ENABLED=1` | Verified private PACK plus manifest, existing SINGLE still active, durable retry/lease behaviour |
| Promote selected disposable SINGLE groups | `TOR_DROP_PACK_CUTOVER_ENABLED=1` | All-edition atomic promotion, exact hashes, and a successful operator rollback to physically reverified SINGLE |
| Begin a separate small queued import | `TOR_CHAPTER_IMPORT_PACK_DIRECT_ENABLED=1` with the authenticated S023 worker | Selected mode durable, can read/edit recovered PACK-only editions, absent or corrupt frame fails closed, start-to-read and retry timings within limits |
| Expand | Increase cohort and concurrency on evidence | Exact reader/auth errors, resource saturation, cost, and restore measurements remain inside the pre-recorded envelope |

Choose explicit numeric staging thresholds before the run using provider
p95/p99, publication wait, verification and authorization failure rates.
Measure old and new paths under the same access policy and workload.
Never expand with missing telemetry, unexplained hash differences,
unplanned rights changes, or a failed restore.

## Stop and recover

Disable direct PACK selection for new jobs first. Previously selected
PACK jobs retain the stored transport and resume using the PACK-capable
worker. Stop the planner and cutover independently to prevent new
moves. The reader must continue serving all active PACK-backed and saved
revisions through incident response.

For SINGLE→PACK cutover, follow [DROP-05c](../contracts/tor-store-drop05c-verified-rollback.md).
Revalidate every original SINGLE receipt and object and perform the
all-edition CAS to a new SINGLE generation. Clear failures through the
same audited operator path.

PACK-direct editions have no original SINGLE. Retain a PACK-capable
binary and disable new PACK work, then recover missing objects from the
verified independent archive. Replacing the binary with a SINGLE-only
version is forbidden while any accessible PACK-only edition depends on
it. Capture both reader paths and publish counters before declaring
recovery. Hold physical cleanup pending a separate retention, provider
and write-fencing decision.

S349 remains the SINGLE authority for interactive work. Reuse the merged
S023/DROP transport, placement and reader contracts; keep older overlap
experiments out of release branches unless their distinct evidence is
reviewed. Required evidence for completion is the actual staged reversal,
provider results and the independent restore run, each tied to an exact
commit and environment.
