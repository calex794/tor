# AUTH-PROFILE-06c — historical privacy boundary: logs, exports and recovery

Scope A-06/E-08/I-11, S004/S015. Static source review at main
`81d16e7f483a0a21d1226b682e8abfe66e96dfc7`; no live host, account,
production database or external archive was inspected. #1365 protects current
public names and reconstructs selected historical system notifications on read.
#1384 and #1386 cover inventory and logical repair of eight actor-event snapshots
and require independent acceptance. Neither is proof of erasure from old media.

## Evidence and concrete change

| Boundary | Reviewed source | Finding |
|---|---|---|
| Future home ingress HTTPS and home receiver | `ops/home-ingress/*-site.nginx.conf.example` | Explicit access_log off; inherited HTTP redirect/default hosts previously lacked it. Old mailbox-bearing paths or query strings could enter access logs. This PR disables request access logs explicitly in every server block; no change to redirect, ACME, TLS or proxy routing. |
| Existing managed edge | `tools/adopt_edge_vhost.py`, `tools/managed_edge_release.py` | Adoption preserves the original vhost and adds managed snippets; it is not evidence that the actual active or inherited Nginx log configuration has been sanitized. The saved vhost is protected and nginx -T is not printed. |
| Failed login observations | `crates/web/src/auth/auth_observation.rs`, `crates/accounts/src/services/auth_observation.rs`, `crates/accounts/src/repositories/auth_observations.rs` | Opt-in keyed HMAC identifiers plus internal user ID and fixed outcomes. No plaintext submitted mailbox in this observation record. These are pseudonyms, not anonymous data. |
| Security retention | `crates/accounts/src/repositories/security_retention.rs` | Bounded cleanup: remove verified IP after 30 days, login/security event rows after 90 days, failed login observations after 7 days, attempt and spray data after one day. These are cleanup thresholds, not guarantees of immediate removal or backup expiry. |
| Request traces | `crates/observability/src/services/trace.rs` | Fixed route/status groups, duration, synthetic trace ID and numeric SQL timing evidence; no raw request path, email or body in this type. Other trace producers and logs are not exhaustively audited. |
| Offline release export | `docs/infra/deploy-offline-export.md` | Signed release export contract excludes credentials, database dump and content keys; this is not an exhaustive audit of every CI artifact or operator export. |
| Logical DB archives | `ops/backup-pg18-offsite.sh`, `docs/infra/i11-offsite-backup.md` | Full logical TOR dump streamed to encrypted restic storage. It legitimately contains private accounts and any historical snapshots existing at capture time. The reviewed script does not implement expiry/prune. |
| Physical PG/WAL archives | `ops/pgbackrest-pitr.conf.example`, `docs/infra/i11-pgbackrest-wal-pitr.md` | Example encrypted repository retains 4 full/6 differential generations, not calendar days. Backup/WAL can contain older row versions and deleted/replaced snapshots. Do not delete individual WAL segments or compromise recovery. |

## Separate operator review (not performed)

1. Inspect effective Nginx server/location inheritance, redirects, default hosts,
   error logs, CDN/proxy and application/journal/PostgreSQL logging settings on
   the actual approved host. Do not print raw configuration, requests,
   credentials or database values into CI/chat. Record only policy evidence,
   aggregate counts and time windows. Nginx error logs may still contain request
   context even with access_log disabled. No blanket disabling of error logs.
2. Inventory rotated logs, old exports and independent backup repositories.
   Verify encryption, access boundaries, restore rights, actual age and retention;
   approve disposal separately under the operational retention and recovery policy.
   Do not log or transmit candidate mailbox text. No automatic log truncation,
   deletion, restic forget/prune or pgBackRest expire is authorized in this PR.
3. For any restore from before privacy repair, validate the restored schema and
   current privacy-safe application revision in isolation before enabling access.
   Repeat inventory and approved logical repair when applicable. Do not expose a
   restored old server to public traffic just to verify a privacy fix.
4. Distinguish logical cleanup, normal MVCC/VACUUM handling and controlled expiry
   of older recovery generations. Erasure of selected live rows does not remove
   old page versions, WAL or independently retained archives. Never delete
   legitimate private account email addresses needed for login/recovery.

Acceptance here: regression tests assert each example vhost disables access logs,
including default and redirect servers, and no existing routing contract changes.
No runtime install, live audit, deployment, backup modification or claim of
complete historical erasure.
