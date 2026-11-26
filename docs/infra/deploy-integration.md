# DEPLOY-08b — integration baseline

Base main: 517fcf5427febf6667e7d2aad326b64b65599b99
Deployment stack: 81c1debd0f63d46bce5f10dd637215700197354c

Preserve all main-only changes by starting from the actual main tree.
Copy deployment-only exact blobs. Reconcile overlapping PG workflow and
architecture guard hunks, signed v2 local migrations with the legacy
root-approved unhealthy-service recovery mode, FREE/Layer real HTTP
and physical PITR coverage, stage-only transport and the shared production
build profile accepted in #1261. Never replace the main recovery or finance
checks with an older branch's versions. No deployment or merge to main.

The previous hosted VM run built Rust successfully but PostgreSQL failed
at systemd startup before first-start tests. Use normal package umask,
standard cluster path, retain old stopped runner PGDATA and report exact
startup diagnostics on failure. The cause is not proven by the old log.
Retry with the real production compiled profile and no S3 provider flags.

v1 unhealthy-VPS recovery remains available before adoption. v2 currently
requires a healthy baseline for normal coordinated update; v2 emergency
outage recovery and independent external DR are not accepted by this PR.
Do not replace the v1 root helper until that operational limitation is
understood. The integrated tree must pass fresh CI and real VM acceptance.

## Confirmed VM startup diagnosis and main synchronization

Run 45819731097 reported PostgreSQL configuration syntax error near .0.
The fixture pg_conftool invocation omitted PostgreSQL-level quoting of
listen_addresses. Quote the string settings and validate using the actual
postgres -C configuration parser before systemd start, then check the live
PG18 settings. This corrects the documented fixture failure, not a live VPS.

Use GitHub's verified merge tree 45d3f24d15912b5caa0447f6a17dae5b496b7818
for main 1035afe11d3c6c0989fdd83aec3f782659c9bc9f plus this PR's prior head.
The merged tree retains RECOVERY-10e-b1's GrantServer TCP HTTP transport,
all DEPLOY07C evidence markers and the physical recovery runbook.

Run 45824045088 confirmed the earlier quoting repair was incomplete:
pg_conftool had added quotes around already quoted values. The architecture
job 45824045113 also found a malformed shell script (duplicated suffix from
a literal replacement of a regex-containing line). Restore the previously
intact script and write the five fixed settings directly into the freshly
created fixture postgresql.conf. Check parser output before systemd starts,
then verify the actual running PG18 settings including shared_buffers bytes.
No production configuration or migration policy is changed.

Synchronize main b88c734a25d36ce018be34ecc9e08b5f3f822c91 using exact non-overlapping blobs; preserve
REUSE-12A/B audience browser helper changes unchanged.

Run 45854126440 confirms PostgreSQL 18 setup and all live settings:
DEPLOY08A_PG_CONFIG_PARSED_OK, DEPLOY08A_REAL_PG18_ACTIVE_OK,
DEPLOY08A_HOSTED_VM_FIXTURE_READY. The actual first-start preflight then
refused the hosted runner's /opt owner/mode. Normalize only /opt itself
inside the guarded disposable fixture, report directory owner/mode and
check every activation parent before starting the acceptance test. Do not
change descendants or relax the production trust guard. First-start and
rollback acceptance remain pending until the next VM job completes.

The 45859640232 VM run confirmed trusted fixture parents and passed first
start preflight/staging including the compiled production profile. It then
refused init-tor-database.sh. Its raw output was intentionally discarded,
so the exact failure was not proven. Source review found the ACL boolean
format mismatch, fixed by native psql boolean columns. Fixed non-secret
stage exits expose future failures without forwarding credential-bearing
output. New main 85768a7b includes the separate forward-only release
policy (#1292); reconcile that policy before final integration, rather
than replacing its workflow and shell guards with old exact-main code.

Synchronize main 85768a7b08e9fb12b5bc71943b9db17fceaf817d including #1292. Preserve all nine
nonoverlapping files as exact main blobs. Reconcile the common workflow
and go-live shell: pinned-main CI evidence, immutable ancestry (active ->
candidate -> main) in both ordinary protocols, and signed v2 shell checks
using a private installed copy of the same shared module. Legacy v1 helper
and guard are unchanged from main; manual first activation remains strict
exact-main. The hosted VM substitutes only fixture release identities,
not production ancestry behavior. No deployment or merge to main.
