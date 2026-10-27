# TOR: production activation on PostgreSQL 18

**Decision, 26 October 2026:** run the actual Rust + PostgreSQL product
at https://pandaolate.ru. The zero-traffic site is not an excuse to keep an
in-memory demonstration. We retain only production essentials: least-privilege
roles, durable schema, real HTTPS, readiness, and a working rollback.

## One-click release (28 October 2026) — current operator path

**Normal updates now need one action:** open
[**TOR — Обновить сайт**](https://github.com/calex794/tor/actions/workflows/tor-one-click-release.yml)
on GitHub Actions, choose branch `main`, press **Run workflow**. No commit
SHA, VPS terminal, `install=true`, SQL commands, or second confirmation are
needed on subsequent updates. GitHub push and PR checks never deploy.

### One-time setup on the dedicated VPS

The original SSH `tor-deploy` user can transfer files but does not have the
root-only PostgreSQL migrator password. For a future one-button deployment,
install a **single root-owned, fixed entry point** once as root on the TOR VPS.
The script refuses unknown release directories, unhealthy TOR, non-PostgreSQL
runtime, missing main proof and unsafe existing sudoers. It does not restart
the site or apply SQLx:

~~~bash
sha="$(curl -fsSL https://api.github.com/repos/calex794/tor/git/ref/heads/main |
  python3 -c 'import json,sys; print(json.load(sys.stdin)["object"]["sha"])')"
curl -fsSL "https://raw.githubusercontent.com/calex794/tor/$sha/ops/bootstrap-one-click-release-vps.sh" \
  -o /root/tor-one-click-setup.sh
bash -n /root/tor-one-click-setup.sh &&
bash /root/tor-one-click-setup.sh "$sha"
~~~

Expected output: `TOR_ONE_CLICK_BOOTSTRAPPED`. The existing
`tor-staging` GitHub Environment secrets (`TOR_DEPLOY_SSH_KEY`,
`TOR_DEPLOY_KNOWN_HOSTS`) and variables (`TOR_DEPLOY_HOST`,
`TOR_DEPLOY_USER=tor-deploy`) are reused; never paste them into chat.

**Important one-time permission change:** `/opt/tor/releases` and
`/opt/tor` become root-owned. New builds are uploaded instead to
`/opt/tor/incoming/<SHA>` and are copied and checksum-verified into a
root-owned release directory. The previous advanced `tor-vps-staging.yml`
workflow is *not* compatible with this setup and is retained only as
historical/diagnostic code: do not use its old `install=true` procedure.
The new button is the supported regular path.

### What pressing the button does

1. Requires current `main` and waits for all checks that actually ran for
   the exact SHA (`Repository rules` is always required). Failed or pending
   checks block the release. If the branch moves during the process, it
   stops instead of deploying a stale version.
2. Builds `tor-web` and `tor-migrate` on GitHub's runner, not on the
   961-MiB VPS, and transfers checksum-verified binaries through verified
   SSH host keys to `tor-deploy`'s inbox.
3. The only passwordless sudo command is the root-owned
   `/usr/local/sbin/tor-one-click-release <SHA>`. It refuses an unhealthy
   production server, verifies the current SHA and PostgreSQL markers,
   copies both binaries to a root-owned directory, and executes the
   pinned root `go-live-postgres-vps.sh` procedure.
4. `go-live-postgres-vps.sh` takes a local PostgreSQL `pg_dump -Fc`
   backup and verifies the archive **before** applying canonical SQLx.
   It checks migration privileges/history, restarts the new Rust binary,
   verifies `/readyz`, catalog, registration/login CSRF and HTTPS.
   On failure it restores the old binary/service settings and verifies
   availability. An already-applied SQLx migration **is not undone**.
5. GitHub reads back the *active* commit, systemd, readiness and the
   PostgreSQL activation marker. Successful output includes
   `TOR_ONE_CLICK_RELEASE_OK <SHA>` and `TOR_DEPLOYED_OK <SHA>`.

### Limits

- Backups are stored locally in `/var/backups/tor`, not automatically
  uploaded to third-party storage. Before accepting important user data,
  provision off-server backups and a real restore drill.
- Database rollback is **not automatic**: replacing the executable
  cannot undo a committed SQLx migration or preserve writes after a
  point-in-time database restore. Migrations must remain backward-compatible
  with the previous executable for a safe automatic application rollback.
- The default release build does not enable `s3-covers,s3-paid`; real
  private S3 features require separate server configuration and validation.
  No fictional paid features are reported as activated by the release.
- Do not run the old stage/install button after this bootstrap. For manual
  disaster recovery use the pinned root procedures below, after verifying
  the backup and the actual VPS state.

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

## Read-only stage preflight before applying the next upgrade

After **all required GitHub Actions checks on the exact current main** succeed,
run [tor VPS staged release](https://github.com/calex794/tor/actions/workflows/tor-vps-staging.yml)
manually with `install=false` and an empty confirmation. A successful staging
job transfers binaries but does not apply SQLx or switch the server. On the
dedicated VPS as root, take the *same full 40-character SHA* from the successful
stage and run the pinned read-only preflight:

~~~bash
sha='<40-character-current-main-SHA-from-successful-stage>'
curl -fsSL "https://raw.githubusercontent.com/calex794/tor/$sha/ops/check-tor-release-vps.sh" -o /root/tor-check-release.sh
bash -n /root/tor-check-release.sh &&
bash /root/tor-check-release.sh "$sha"
~~~

This preflight reads the active release, current server readiness, PostgreSQL
SQLx history, root-verified schema markers, staged migrator checksum, current
GitHub main SHA, and candidate migration count. It **never** applies migrations,
restarts anything, switches `/opt/tor/current`, or reads plaintext secrets.
A healthy but outdated database produces
`TOR_PREFLIGHT_STAGE_OK_MIGRATIONS_REQUIRED` with the number of pending versions.
A stale candidate or missing stage produces `TOR_PREFLIGHT_REFUSED`. Exit 0
means the staged candidate passed these *read-only* checks — not that it is
safe to deploy: verified off-server backup, rollback compatibility, full CI,
operator approval, S3 credentials and required feature flags remain separate.

Do not launch the root go-live from an obsolete staging run. The root go-live
performs its own GitHub main checks before and after migrating; if `main`
changes during migration, the old executable remains active while the
already-applied SQLx migrations persist. Stop merges during the brief actual
release window and run the release only from a verified, stable SHA.

## Controlled release after the first switch — 27 October 2026

**Current I-line safety decision:** merging into `main` runs tests but never invokes deployment or SSH. The former `workflow_run` trigger and `TOR_AUTO_DEPLOY` flag were removed from `.github/workflows/tor-vps-staging.yml`. The old auto-deploy instruction below is superseded.

To stage a candidate, run [tor VPS staged release](https://github.com/calex794/tor/actions/workflows/tor-vps-staging.yml) manually from `main` with `install=false` and an **empty** `confirm_sha`. The stage transfers only the SHA-pinned binaries; it does not switch the running service.

For `install=true`, supply `confirm_sha` equal to the **full 40-character `main` SHA of this workflow run**. The validation fails before checkout/build/SSH if the value differs, is truncated, or if a stage-only run supplied a confirmation. The existing root-verified SQLx schema and PG activation markers, healthy previous service, current-main recheck, rollback verification and HTTPS checks still apply. A missing migration marker prevents activation, even with correct confirmation.

The root-only go-live now also compares its pinned SHA with GitHub's exact
`refs/heads/main` **before migrating anything** and again before changing
the systemd service. This requires `python3` and access to
`https://api.github.com` on the VPS; verification fails closed if they are
missing or GitHub main has changed. If main changes *after* SQLx ran, the
old binary stays active but the added migrations persist: do not mistake
a refused switch for a database rollback. A fresh staging run is required
for the new main SHA.

The operator's version-pinned `go-live-postgres-vps.sh` remains the only procedure for applying new migrations. To advance an existing PostgreSQL schema, first perform backup and root-only migration/proof, then manually dispatch installation against the exact approved `main` SHA. Do not use `TOR_AUTO_DEPLOY=true`; it is ignored by the current workflow.

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

## Schema-gated manual releases (updated 27 October 2026)

The previous automatic stage could activate a new binary without applying its
new SQLx migrations. In particular, B-06 requires migration
`202610260007_chapter_working_drafts.sql`; the new binary's `/readyz`
now fails closed if that migration or its working table is missing.

Staging always transfers the SHA-pinned binary and migrator. Before any
manually approved activation, the CI job compares the latest
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
Before manual activation, CI verifies this marker, the non-demo systemd
override and a healthy local /readyz. This prevents a successful schema
migration from accidentally authorizing a demo-mode deployment.

If the marker is absent or does not exactly match, the candidate can be
**staged but is never activated**. The operator applies migrations
through the standard go-live procedure using the same staged Git SHA; its
normal `/readyz`, HTTP and HTTPS checks still control the switch. A marker
is evidence of the verified migration version, not evidence that a specific
binary was activated. Only a logged `Active tor commit: <SHA>` or
`TOR_PRODUCTION_OK <SHA>` establishes deployment.

Manual installation also rechecks the exact current `main` SHA immediately before
the service switch. If another PR merged while compilation was running,
the obsolete candidate is rejected. This prevents a stale staged binary
from overwriting newer code.

Do not weaken the `tor_app` role to allow migrations, temporary tables, or
DDL. The migration runner uses `tor_migrator` under the operator's restricted
VPS procedure; no secrets are stored in GitHub releases or Notion.

## Подтверждение отката и восстановления после сбоя — 26 октября 2026

[Выпуск Actions №40382065303](https://github.com/calex794/tor/actions/runs/40382065303) доставил сборку `9f31dab2`, но после смены версии сервис не ответил на `127.0.0.1:3000/readyz`. Старый сценарий заменил ссылку `/opt/tor/current` обратно и попробовал перезапустить `tor-web.service`, однако сам перезапуск сообщил об ошибке. **Смена ссылки не доказывает восстановления сайта**. По одному журналу Actions нельзя утверждать, что прежняя версия работала после отказа.

Для следующих выпусков автоматический переход выполняется **только если прежняя версия уже активна и отвечает `ok` на `/readyz`**. После переключения новая версия должна пройти и `/readyz`, и открытие настоящей страницы регистрации с полем CSRF; положительный ответ демонстрационного сервера недостаточен. При отказе система возвращает предыдущую ссылку, делает повторный запуск и отдельно проверяет HTTP. В журнале различаются `ROLLBACK_VERIFIED` (сайт восстановлен) и `ROLLBACK_UNVERIFIED` (оператор должен вмешаться). Состояние systemd выводится через `systemctl show` без раскрытия секретов и полного журнала. Четыре испытания с подставными сервисами выполняются в CI без доступа к рабочему VPS.

**Действие оператора, только чтение**, перед новой попыткой (секреты/сырые строки журнала не публиковать):

~~~bash
readlink /opt/tor/current
systemctl show tor-web.service --no-pager -p ActiveState -p SubState -p Result -p ExecMainCode -p ExecMainStatus
curl -fsS --max-time 3 http://127.0.0.1:3000/readyz
~~~

Если ответ не `ok`, посмотреть `journalctl -u tor-web.service` **локально на VPS**, проверить текущую миграцию PostgreSQL и запуск из прошлой версии. Автоматическая выкладка не должна использоваться как способ чинить неизвестное нерабочее состояние. Применённые SQLx-миграции откатом программы не отменяются; для базы остаются отдельные резервная копия и проверка обратной совместимости.
