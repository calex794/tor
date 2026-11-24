# TOR Recovery / I-11 / S061 — RECOVERY-02: isolated clean-host workspace

Status: executable workspace preparation only; no package installation,
service startup, real restore, public activation or operational DR evidence.
Tracking: [#739](https://github.com/calex794/tor/issues/739).

## Why not run the usual bootstrap?

Existing bootstrap-postgresql-vps.sh, init-tor-database.sh and
one-click release procedures assume a running TOR or a normal go-live
target. A disaster restore needs a separate, empty physical PGDATA
directory, not a new initialized main cluster. Never point recovery at an
existing production cluster or initialize over a physical backup.

## Operator prerequisites

- A freshly installed, independently approved Ubuntu 24.04 host, not the
  failed source server. Verify its identity through the provider console
  and compare independently retained source and destination /etc/machine-id.
  Machine IDs are a local guard, not proof of separate infrastructure.
- No live TOR installation, production network routes, payments, email,
  production S3 write credentials or DNS changes. Do not clone a compromised
  VM image. Verify firewall and cloud IAM separately.
- Install trusted PostgreSQL 18, pgBackRest, rclone, age, OpenSSH and
  Python 3 packages; ensure postgres is an unprivileged OS account.
  Install/configure package repositories via the trusted OS/provider path.
  The preparation tool does not invoke apt or existing TOR bootstraps.
  Any package-created default cluster is outside the recovery workspace.
- Retain the verified offline tool itself independently before the incident.

## Prepare

Run this on the approved new host as root, with machine IDs taken from
independent records. Without --prepare it only checks prerequisites.

```bash
sudo python3 tools/tor_recovery_host.py \
  --source-machine-id "$SOURCE_MACHINE_ID" \
  --target-machine-id "$TARGET_MACHINE_ID" \
  --pg-port 15432 --prepare
```

The fixed /srv/tor-recovery path must not exist (even a symlink). The tool
creates a root-owned, postgres-traversable workspace, empty pgdata,
private pgsocket and pglog directories owned by postgres, and private
operator-only release, objects and evidence directories. It creates
isolated PostgreSQL configuration under config/ with no TCP listener,
port 15432 by default, peer-only postgres socket access, archive_mode off
and no inherited preload libraries. It does not start PostgreSQL or TOR.

Never reuse or automatically delete an existing workspace. On a failed
or interrupted preparation, inspect the partial directory separately.
The command cannot claim recovery acceptance; it only prepares the target.
The offline release verifier from RECOVERY-01 may stage the approved release
into the prepared release directory after that slice is accepted.

## Next: restore exact checkpoint, not latest

Prepare a reviewed recovery-only pgBackRest config with the selected
offsite repository and exact backup-set/target time, read-only credentials,
pg1-path=/srv/tor-recovery/pgdata, isolated port and socket. Do not restore
into /var/lib/postgresql/18/main or run initdb on this empty pgdata.

RECOVERY-03 must supply the actual restore and startup guard. Restored
postgresql.auto.conf, include files, recovery settings, extensions and
archive_command may contain settings from the lost host; do NOT start
PostgreSQL merely because the separate config exists. Inspect restored
files, enforce socket-only/no archive-push and independently verify all
startup-critical overrides and the exact recovery target before starting.
The later app acceptance must additionally recreate restricted local
roles, use fresh credentials, verify schema and paid/free rights, and
ensure no live side effects. No DNS or traffic cutover until approval.

Use [independent DR acceptance](i11-independent-dr-acceptance.md) for
external backup/keys and measured RPO/RTO, not this workspace tool.
