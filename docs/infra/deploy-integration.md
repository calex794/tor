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
