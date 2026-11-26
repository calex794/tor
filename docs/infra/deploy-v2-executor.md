# DEPLOY-05a — root executor v2

The existing one-click shell remains v1 until the trusted installer and
workflow transport are switched together. This PR supplies the application
adapter and fixed v2 executor, not a new sudoers command.

The signed bundle includes both existing shell scripts and generated
tor-migrate.sha256. No raw GitHub script download in the explicit v2
migration branch. The existing database backup, SQLx, current-main checks,
auth/catalog/HTTPS acceptance and settings validation are reused.
The v2 preflight requires the existing PG drop-in and marker. An active or
enabled optional Trace Store is refused rather than stopped.

The executor takes one full SHA, verifies current main, acquires the
existing release lock, stages the exact signed inbox copy, and invokes
the durable coordinator. Local readiness AND HTTPS readiness must succeed;
MainPID and actual /proc/PID/exe inode must match the expected executable.
The synchronous child inherits the release-lock descriptor, so a surviving
migration cannot race another update after parent termination.
On failure the coordinator restores edge and unit first, then the previous
application and verifies its live binary and readiness. SQLx schema and
all data are retained. No automatic down-migrations.

Explicit root recovery (--recover) does not require the latest main SHA.
The future fixed sudo wrapper MUST accept exactly one SHA, never recovery
or arbitrary args. No new sudoers permission or deployment in this PR.
Installer of trusted modules, capability negotiation, workflow v2 transport,
boot-time recovery and real systemd/Rust/PG acceptance remain next steps.
A previously broken site requires separate RECOVER flow; normal update
requires a healthy baseline. Tests exercise a real lock, file system,
process identity and injected commands, not an actual production restart.
