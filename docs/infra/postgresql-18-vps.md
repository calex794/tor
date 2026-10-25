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
