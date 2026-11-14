# I-11 / DR-04: WAL archive health guard (operator-only)

Status: **not installed, not scheduled, not validated against an independent S3**. This adds a read-only host probe and disposable tests; it cannot certify RPO/RTO or replace actual PITR drills. See #739, DR-02 `i11-pgbackrest-wal-pitr.md`, DR-03 #836, TOR-HOME-01.

## Scope and activation boundary

- `ops/check-pgbackrest-archive-health.py` must be copied manually to `/usr/local/libexec/tor/` on an approved PostgreSQL 18 host. Root access and a local `tor` database are required. It refuses CI and command-line arguments. It never changes PG settings, switches WAL, writes to S3, exposes archive credentials, or restores a database.
- It queries only the trusted local socket, enforces the known cluster path and pgBackRest stanza, checks S3-backed backup catalog reachability, last physical backup age (maximum 36 hours), latest failed/successful WAL archive events, pending `.ready` status files (limit 10 minutes), and local WAL-disk free percentage (minimum 25%). No raw pgBackRest errors or secrets are printed.
- Exit `0` emits a JSON `TOR_PITR_HEALTH` line for machine parsing; exit `2` emits a fixed `TOR_PITR_HEALTH_CRITICAL` reason. Add the systemd service/timer examples **only after operator approval** and connect failed-unit or journal alerts to an external monitor. No alert destination or credentials are configured here. A timer without an external watcher does not meet operational alert acceptance.
- Templates are inert: neither the executable nor systemd units are installed or enabled by this PR. There is no production cron, remote S3 access in CI, or application migration.

## What the monitor cannot prove

On an idle database the last completed archived WAL can be old without an outage. `idle_rpo_unproven=true` is explicitly reported in that case. A green probe means the inspected state has no detected failure, **not** that all transactions reached S3 within 15 minutes. Real offsite recovery and fault injection must verify that target; use a separate safe WAL heartbeat/restore rehearsal only when explicitly authorized.

## Independent acceptance still needed

1. On an approved staging PostgreSQL 18 using separate test credentials, verify `pgBackRest` repository is truly outside the host and that S3 object versioning, Object Lock/IAM and retention match the documented plan. Check that 36-hour backup freshness is appropriate after actual scheduling.
2. Simulate loss of network/S3, failed WAL push, stale pending `.ready`, full PG WAL disk and missing backup chain; confirm the probe fails, a remote alert is delivered and no WAL is discarded.
3. On a separate disposable host, restore the latest full/differential backup and WAL to a known point. Restore the independently archived private S3 files and the separately held encryption keys; test purchased V1, refunds, and access denials. Measure elapsed RPO/RTO and retain a redacted evidence report.
4. Do not mark S061 or issue #739 complete or enable real money solely because this monitoring script passes unit tests.

Unit tests: `python3 -m unittest discover -s tests/architecture -p 'test_i11_archive_health.py' -v` (zero database/cloud credentials).
