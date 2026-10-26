# TOR: production activation on PostgreSQL 18

**Decision, 26 October 2026:** run the actual Rust + PostgreSQL product
at https://pandaolate.ru. The zero-traffic site is not an excuse to keep an
in-memory demonstration. We retain only production essentials: least-privilege
roles, durable schema, real HTTPS, readiness, and a working rollback.

## Verified current VPS state

- PostgreSQL 18.6 running, 127.0.0.1:5432 only; 1 vCPU / 961 MiB RAM.
- DB tor owned by tor_migrator; tor_app / tor_migrator have connection
  limits 6 / 3 and neither is a superuser, createdb or createrole.
- /etc/tor/tor-app.env and tor-migrator.env are 0600 root:root.
- Last owner verification: **0 public domain tables**, still RAM demo
  in systemd; ~562 MiB MemAvailable. The server is not yet PG-backed.
- In GitHub TOR: nine canonical SQLx migrations, tor-migrate binary,
  PgCatalog, PgReader, PgAccounts, and the real PostgreSQL 18 CI suite.

## One-time switch to production

1. Open https://github.com/calex794/tor/actions/workflows/tor-vps-staging.yml
   and run it manually on main, with install=false. Confirm SUCCESS.
   This sends verified tor-web and tor-migrate executables to a SHA-named
   release folder on the VPS. No demo candidate is run and no active
   site or schema is changed by this stage.
2. Copy the exact **40-character main commit SHA** of that successful
   workflow into the operator commands (under root on the TOR VPS):

~~~bash
sha='<FULL_SHA_FROM_SUCCESSFUL_MAIN_STAGE>'
curl -fsSL "https://raw.githubusercontent.com/calex794/tor/$sha/ops/go-live-postgres-vps.sh" -o /root/tor-go-live.sh
bash -n /root/tor-go-live.sh &&
bash /root/tor-go-live.sh "$sha"
~~~

The one-shot operator script checks real PostgreSQL, SCRAM tor_app
credentials, both binary checksums, schema owner and the release location.
If the schema is empty, it calls the pinned canonical SQLx runner,
creates a local compressed backup first, and verifies nine donor SQLx versions plus any additive TOR versions
and at least 17 domain tables. It then installs a systemd override using
EnvironmentFile=/etc/tor/tor-app.env (read by systemd PID 1), sets
TOR_DEMO_MODE=0, TOR_AUTH_ENABLED=1 and
TOR_PUBLIC_ORIGIN=https://pandaolate.ru, and atomically switches the
versioned executable. Real database-backed readiness, registration/login
forms with CSRF, catalog, API and local Nginx HTTPS are checked.

**If production cutover fails**, the script restores the previous service
config and Rust binary. SQLx schema/data remain in PostgreSQL and are
not erased by binary rollback.

Acceptance markers:

~~~text
MIGRATION_HISTORY_ACCESS_OK
MIGRATIONS_OK: <N>/<N> SQLx versions, <TABLE_COUNT> domain tables
TOR_PRODUCTION_OK <SHA>
POSTGRES_READY_OK (<N>/<N> migrations, tor_app read-only schema history)
AUTH_PAGES_OK /register /login
HTTPS_READY_OK https://pandaolate.ru/readyz
REAL_DATA_MODE: PostgreSQL; no RAM demo
~~~

The catalog may be empty because no actual books have been entered yet.
**Do not insert fake production records.** Deliver authoring and real
content iteratively using the existing nine TOR workstreams.

**Production implementation note:** the cover image worker introduced
migration 202610260001 (four additional tables), so the release runner must
verify the embedded SQLx migration list rather than assuming exactly nine
total versions or 17 total tables. The CLI reports TOR_SQLX_APPLIED_OK N/N;
future additive migrations use the same mechanism.

## Fast, normal CI/CD after the first switch

Enable repository Actions variable TOR_AUTO_DEPLOY=true after the
first successful PG-backed activation. Auto-deploy is now triggered
only by a successful TOR PostgreSQL runtime workflow on main,
not the old Rust S0 demo workflow. The release stage packages
binaries without rerunning the whole test suite. The active service
remains configured for real PostgreSQL on every subsequent restart.

**Next engineering task:** integrate the migration upgrade process
for future schema version 010+ into the normal deploy path.
The SQLx runner recognizes all current additive migrations and verifies the
nine immutable historical donor versions. Do not silently deploy breaking DB changes:
binary rollback does not roll back DDL.

**Actual production limits:** accounts, catalog and public reading
are wired to PostgreSQL; full authoring, paid chapter delivery,
payment providers, ads, RIL and other domains require their own
shipped implementation and acceptance. Enable real off-server
backups with a restoration test before important user data.
The external URL book collector S024/F021 remains frozen.

## Schema-gated automatic releases (October 2026)

The previous automatic stage could activate a new binary without applying its
new SQLx migrations. In particular, B-06 requires migration
`202610260007_chapter_working_drafts.sql`; the new binary's `/readyz`
now fails closed if that migration or its working table is missing.

Staging always transfers the SHA-pinned binary and migrator. Before any
automatic or workflow-dispatch activation, the CI job compares the latest
SQLx version in the candidate's `migrations/` directory with the last version
recorded in `/var/lib/tor-release/schema-version` on the VPS. This marker is
written atomically by the **root-only**, version-pinned
`ops/go-live-postgres-vps.sh` after a successful migrator run and full
SQLx history/role verification. The marker's directory is root-owned and
not writable by the SSH deployment account.

The database marker alone is insufficient: it is written before the
application is switched and therefore could survive a failed first cutover.
The operator script now also writes the root-owned
`/var/lib/tor-release/pg-activated` marker **only after** the real
PostgreSQL application passes /readyz, login/registration and HTTPS checks.
Before auto-activation, CI verifies this marker, the non-demo systemd
override and a healthy local /readyz. This prevents a successful schema
migration from accidentally authorizing a demo-mode deployment.

If the marker is absent or does not exactly match, the candidate can be
**staged but is never automatically activated**. The operator applies migrations
through the standard go-live procedure using the same staged Git SHA; its
normal `/readyz`, HTTP and HTTPS checks still control the switch. A marker
is evidence of the verified migration version, not evidence that a specific
binary was activated. Only a logged `Active tor commit: <SHA>` or
`TOR_PRODUCTION_OK <SHA>` establishes deployment.

Auto-install also rechecks the exact current `main` SHA immediately before
the service switch. If another PR merged while compilation was running,
the obsolete candidate is rejected. This prevents a stale staged binary
from overwriting newer code.

Do not weaken the `tor_app` role to allow migrations, temporary tables, or
DDL. The migration runner uses `tor_migrator` under the operator's restricted
VPS procedure; no secrets are stored in GitHub releases or Notion.
