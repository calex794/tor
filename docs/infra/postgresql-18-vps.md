# PostgreSQL 18 на VPS TOR (1 vCPU, 961 МиБ RAM)

**Задача линии I-05.** Поднять постоянное хранилище рядом с `tor-web` без Docker и без остановки HTTP-сервера. По проверке владельца от 26.10.2026 PostgreSQL/psql отсутствует, порт 5432 свободен, MemAvailable 585 МиБ, диск 17 ГБ, `tor-web.service` включён и активен.

**Источники:** [официальная установка для Ubuntu](https://www.postgresql.org/download/linux/ubuntu/), [ресурсные параметры PG18](https://www.postgresql.org/docs/18/runtime-config-resource.html), [канонические TOR DDL и порядок миграций](../roadmap/data-and-events.md). Не использовать `apt install postgresql` без версии: Ubuntu 24.04 обычно устанавливает PG16. Стабильную PG18 брать из официального репозитория PGDG, `noble-pgdg` с `Signed-By`.

## Установка на текущем выделенном VPS

Под root на `vm15714674`:

```bash
curl -fsSL \
  https://raw.githubusercontent.com/calex794/tor/main/ops/bootstrap-postgresql-vps.sh \
  -o /root/tor-postgresql-bootstrap.sh
bash -n /root/tor-postgresql-bootstrap.sh &&
bash /root/tor-postgresql-bootstrap.sh
```

Сначала можно открыть скачанный текст в редакторе. Исполняемый файл хранится в [ops/bootstrap-postgresql-vps.sh](../../ops/bootstrap-postgresql-vps.sh), а его SHA фиксируется по конкретному коммиту в операторском сообщении. Скрипт:
1. Проверяет root, Ubuntu 24.04 (noble), доступную память ≥256 МиБ, отсутствие открытого наружу TCP 5432.
2. Устанавливает `postgresql-common`, официальный ключ PGDG/ `noble-pgdg` apt source и **конкретный** пакет `postgresql-18` без общего `apt upgrade` или перезагрузки.
3. Создаёт `18/main` только при отсутствии, сохраняет копию `postgresql.conf.before-tor.*`, применяет низкопамятную конфигурацию и перезапускает **только PostgreSQL 18**. Не трогает `tor-web`, Nginx, Docker и systemd unit Rust.
4. Выполняет `SELECT 1` от системного `postgres`, проверяет отсутствие публичного listener 5432, выводит состояние PG, потребление RAM/диска, проверку действующего Rust `/readyz`.

**Параметры начального запуска, не обещание производительности:** `listen_addresses=127.0.0.1`, `max_connections=20`, `shared_buffers=64MB`, `work_mem=2MB`, `maintenance_work_mem=32MB`, `autovacuum_max_workers=1`, `max_parallel_workers=0`, `max_worker_processes=2`, `effective_cache_size=256MB`, `jit=off`. При реальной нагрузке проводить отдельные измерения `MemAvailable`, RSS, latency/p95/ошибок, PG I/O, swap pressure и пересматривать параметры.

**Установка создаёт только кластер PostgreSQL и системную роль `postgres`.** Базу `tor`, `tor_app`, отдельную роль мигратора, пароли и таблицы скрипт не создаёт; их завести отдельным ограниченным шагом по владельцу линии I и `docs/roadmap/data-and-events.md`. До этого `tor-web` остаётся read-only RAM-demo, `/readyz` **не проверяет PG**.

## Приёмочные проверки после установки

```bash
pg_lsclusters
systemctl is-active postgresql@18-main.service
runuser -u postgres -- psql -X -Atqc 'SELECT 1'
ss -lntp | grep ':5432'
free -h
df -h /
systemctl is-active tor-web.service
curl -fsS http://127.0.0.1:3000/readyz
```

Ожидаем: cluster `18 main online`, PG `active`, SQL `1`, TCP только `127.0.0.1:5432`, Rust `active` и `ok`. Не публиковать 5432 через UFW/Nginx. Значения RAM до/после установки записать в Notion/Tela для измерения ресурса, а не угадывать потребление процесса по сумме RSS.

**Отказ / давление памяти:** `systemctl stop postgresql@18-main.service` останавливает только БД с сохранением её файлов; сайт S0 остаётся на RAM. Если проблема в параметрах, исходный `postgresql.conf.before-tor.*` сохранился. Не удалять `/var/lib/postgresql` и `/etc/postgresql` при отладке. Нельзя делать реальную финансовую запись или обещать сохранность пользовательских данных без `pg_dump` вне VPS и теста восстановления.

## Следующая инженерная итерация I-03/I-05 → B/F/C

1. В `tor/migrations` перенести девять **неизменённых** SQL-миграций донора `calex794/bokarust` в каноническом порядке, закрепить checksum, SQLx/migrator и два сценария fresh+upgrade после 009.
2. Поднять PG18 service в GitHub Actions, автоматический применение миграций на пустой тестовой БД и отрицательные проверки ACL/версий.
3. На VPS создать независимую миграционную и app-роли/БД `tor`, держать секреты только в защищённых server env/файле, выдать минимум прав. Не подмешивать реальные данные из старой YDB.
4. Реальная вертикаль **HTTP → SQLx → PostgreSQL → Nginx/браузер**: публичный каталог, книга и опубликованная глава, 404 для черновика; рестарт не теряет данные. После этого `/readyz` обязан проверять SELECT 1 и версию миграции.
5. Бекап `pg_dump` на **внешнее** хранилище + проверка восстановления; только потом реальные пользователи, финансы, S3-paid и остальные линии A–H.

Автоматический сбор книг/сканирование URL остаётся замороженным (S024/F021, G-03), не выполнять.


## Создание БД `tor` и двух изолированных ролей

**Выполнять только после подтверждения `postgresql@18-main.service` active.** При разрыве SSH проверять состояние до повторного запуска. Инициализатор запускается из root, повторный запуск использует уже сохранённые пароли, не генерирует новые.

```bash
curl -fsSL https://raw.githubusercontent.com/calex794/tor/main/ops/init-tor-database.sh \
  -o /root/tor-db-init.sh
bash -n /root/tor-db-init.sh && bash /root/tor-db-init.sh
```

Что делает: проверяет PostgreSQL 18, SCRAM и локальное `listen_addresses`, заводит `tor_migrator` (владелец БД `tor`, право создавать таблицы) и `tor_app` (только SQL DML по разрешённым объектам, без CREATE/superuser). Пароли случайные 32 байта, генерируются **только на VPS**, сохраняются в `/etc/tor/tor-app.env` и `/etc/tor/tor-migrator.env` с правами `root:root 0600` и содержат `DATABASE_URL`. Файлы и значения паролей нельзя отправлять в чат, GitHub, Tela или Notion. Проверяются настоящие TCP/SCRAM логины и отказ `tor_app` в CREATE. `tor-web.service` пока работает в RAM-деморежиме, не читает эти файлы.

**Зафиксировать в отчёте только:** `tor_app|f|f|f|6`, `tor_migrator|f|f|f|3`, состояние PostgreSQL, память и права 600 на два файла. **Секреты не показывать** (не запускать `cat /etc/tor/*.env` и не включать `set -x`).

**Следующее:** перенос девяти SQLx миграций от [bokarust](https://github.com/calex794/bokarust/tree/main/crates/adapters/migrations), единый мигратор с CI PostgreSQL 18, schema-compatibility и bootstrap-backup. Никаких фиктивных pg-связей к демо-HTTP.



## 26.10.2026 — verified PostgreSQL + manual SQLx schema handoff

**Operator-verifiable facts (VPS owner output):** PostgreSQL 18.6 running; \`tor\` owner \`tor_migrator\`; app role \`tor_app|f|f|f|6\`, migrator \`tor_migrator|f|f|f|3\`. \`tor-app.env\` and \`tor-migrator.env\` are root:root with mode 600. **Public domain table count = 0.** Rust/Nginx still serve the old in-memory S0 demo; this says nothing about readiness of the real account/reader platform.

**Repo implementation already exists:** \`crates/migrations/migrate.rs\` invokes \`sqlx::migrate!\` for the nine original \`migrations/*.sql\` files, refuses any PostgreSQL role other than \`tor_migrator\` or any database except \`tor\`, \`tor_ci\`, \`tor_test\`. \`PgCatalog\` verifies version \`202610130003\` in \`_sqlx_migrations\` and tests its table permissions. [Real PostgreSQL 18 CI](../../.github/workflows/tor-pg-runtime.yml) runs migrations twice and forbids the app role from applying them.

**On the tiny VPS do not build Rust.** The [manual-only deployment workflow](../../.github/workflows/tor-vps-staging.yml) now builds \`tor-migrate\` on a GitHub runner alongside \`tor-web\`, sends both binaries over the verified SSH channel into \`/opt/tor/releases/<SHA>/\` and verifies a SHA-256 checksum file. *The workflow NEVER executes the migration.* A dispatch with \`install=false\` stages the binaries and smoke-tests the new candidate on loopback port 3099 while leaving \`/opt/tor/current\` untouched.

### Operator's first migration (empty TOR database)

1. In [GitHub Actions / tor VPS staged release](https://github.com/calex794/tor/actions/workflows/tor-vps-staging.yml) choose **Run workflow → main → install=false**. Require workflow success. Note the commit SHA shown by that exact run. Never use an arbitrary artifact from a PR.
2. Under root on the TOR VPS verify that staged release contains the migration executable and its SHA-256 manifest:

   \`\`\`bash
   sha="<SUCCESSFUL_MAIN_RUN_COMMIT_SHA>"
   ls -l "/opt/tor/releases/$sha/tor-migrate" "/opt/tor/releases/$sha/tor-migrate.sha256"
   (cd "/opt/tor/releases/$sha" && sha256sum -c tor-migrate.sha256)
   \`\`\`
3. Obtain the operator script from the **same pinned main SHA** so it is auditable, check syntax, run with the explicit SHA (script must run under root; never supply credentials on command line or in chat):

   \`\`\`bash
   curl -fsSL "https://raw.githubusercontent.com/calex794/tor/$sha/ops/apply-tor-migrations-vps.sh" \
     -o /root/tor-first-migrations.sh
   bash -n /root/tor-first-migrations.sh &&
   bash /root/tor-first-migrations.sh "$sha"
   \`\`\`

The operator script refuses invalid SHA, missing binary/checksum, wrong PG instance/owner/role/secrets, already populated public schema, inactive web/DB. Before any DDL it creates a **root-readable only, local** compressed \`pg_dump -Fc\` in \`/var/backups/tor/\` and checks that \`pg_restore -l\` can read it. It then executes the trusted SQLx binary as \`tor_migrator\` against **only** local \`tor\`; afterward it requires exactly **nine successful \`_sqlx_migrations\` rows and 17 domain tables**, revokes runtime INSERT/UPDATE/DELETE on `_sqlx_migrations` while retaining SELECT for readiness, plus a still healthy demo site. The command is not an HTTP deploy and never replaces the current Rust service.

**Acceptance markers:**

\`\`\`text
MIGRATION_HISTORY_ACCESS_OK: tor_app can SELECT but cannot modify migration history
MIGRATIONS_OK: 9/9 SQLx versions, 17 domain tables
BACKUP_OK: /var/backups/tor/tor-before-... (local only; off-server backup still required)
TOR_DEMO_HEALTH_OK
\`\`\`

**Do not confuse local dump with durable backup:** a copy to a *separate storage failure domain* and tested restore remain prerequisites to real users, paid chapters and finances. If the script fails, preserve both the dump and PostgreSQL state and inspect the error before retrying; schema/binary rollback are **not** the same operation. Never run the disposable CI \`tests/postgres/verify-migrations.sh\` on the VPS: it expects root-like test credentials and creates/drops test DBs.

After schema activation, do a *separate* runtime rollout: configure a restricted environment file readable by the \`tor-deploy\` systemd account (never expose migrator credentials to the app), remove \`TOR_DEMO_MODE=1\`, enable real \`DATABASE_URL\`, verify DB-backed \`/readyz\`, public catalog/book/chapter and negative authorization cases, perform restart/recovery checks, and then add off-site backups. Don't simply \`EnvironmentFile=/etc/tor/tor-app.env\` without accounting for that file's root-only directory and systemd security model.
