# TOR: подготовка существующего VPS к первому визуальному запуску

> **Этот документ описывает исторический первый запуск S0.** Для рабочего сайта с настоящим PostgreSQL, регистрацией и читательскими данными применять [production activation](tor-production-activation.md), а не `TOR_DEMO_MODE=1`. Релиз GitHub после первого включения должен проходить проверку PostgreSQL.

**Актуализация 26.10.2026:** S0 Rust RAM-demo работает под systemd на `pandaolate.ru`, Nginx HTTPS → `127.0.0.1:3000`, активный релиз `c2c0429b61f06df8dd3372452f23cfd7c15ff87a`. Владелец подтвердил открытие сайта с внешнего компьютера, пять маршрутов HTTP 200, `X-Robots-Tag: noindex, nofollow`, MemAvailable ~603 МиБ. Ниже историческая инструкция первого запуска; команды на VPS выполнял оператор, а не ChatGPT. **Полная платформа и PostgreSQL ещё не внедрены.**

## 1. Убрать оставшийся мусор только старого сайта

```bash
cd /root || exit 1
docker volume rm books_platform_static-assets
docker image ls --format '{{.Repository}}:{{.Tag}}  {{.Size}}'
docker system df
du -sh /opt/books_platform
findmnt -R /opt/books_platform
```

Если вывод `docker image ls` подтверждает, что **все** оставшиеся образы можно удалить и `docker ps -a` по-прежнему пуст:

```bash
docker image prune -a -f
docker builder prune -f
docker system df
df -h /
free -h
```

Не удалять `/opt/books_platform` без осмотра файлов и монтирований. Удаление контейнеров не удаляет внешние YDB/S3. Удаление их ключей/ресурсов планировать отдельно. Системный nginx/SSH не трогать.

После сверки `docker ps -a` == 0 можно **по выбору** остановить и отключить Docker/containerd ради памяти, так как TOR работает нативным Rust-бинарником:

```bash
systemctl disable --now docker.service docker.socket containerd.service
free -h
```

Это обратимо через `systemctl enable --now docker.service containerd.service` и не удаляет образы. Не применять, если Docker нужен для других работ.

## 2. Первый безопасный пользователь развёртывания

На VPS от root выполнить после очистки:

```bash
id tor-deploy || useradd -m -s /bin/bash tor-deploy
install -d -o tor-deploy -g tor-deploy -m 0750 /opt/tor /opt/tor/releases
install -d -o tor-deploy -g tor-deploy -m 0700 /home/tor-deploy/.ssh
```

Создаём ключ **не на Windows, а прямо из консоли VPS**. Приватный ключ после создания переносим непосредственно в защищённый GitHub Secret, публичный — в `authorized_keys` пользователя `tor-deploy`. В чат закрытый ключ не отправлять.

После слияния [скрипта подготовки](../../ops/bootstrap-tor-deploy.sh) с `main` выполнить на VPS под `root`:

```bash
curl -fsSL https://raw.githubusercontent.com/calex794/tor/main/ops/bootstrap-tor-deploy.sh -o /root/tor-bootstrap.sh
bash -n /root/tor-bootstrap.sh
bash /root/tor-bootstrap.sh
```

Скрипт создаёт закрытый ключ в `/root/.config/tor/tor_deploy_ed25519` (права 600), добавляет открытый ключ в `/home/tor-deploy/.ssh/authorized_keys` и формирует `/root/.config/tor/known_hosts` на основе действительного ED25519 host key самого VPS. Не трогает системный Nginx. Публичный IP определяется через `api.ipify.org`; проверить, что это адрес VPS из панели провайдера. Если определение не удалось, запустить с `TOR_DEPLOY_HOST=ПРОВЕРЕННЫЙ_IP bash /root/tor-bootstrap.sh`.

Затем на [GitHub → tor → Settings → Environments](https://github.com/calex794/tor/settings/environments) создать `tor-staging` и внести:

- **Environment Secret `TOR_DEPLOY_SSH_KEY`** — содержимое закрытого файла `/root/.config/tor/tor_deploy_ed25519`, копировать напрямую из защищённой SSH-консоли **только** в GitHub Settings.
- **Environment Secret `TOR_DEPLOY_KNOWN_HOSTS`** — содержимое `/root/.config/tor/known_hosts`.
- **Environment Variable `TOR_DEPLOY_HOST`** — проверенный публичный IP/домен из `known_hosts`.
- **Environment Variable `TOR_DEPLOY_USER`** — `tor-deploy`.
- **Repository Variable `TOR_AUTO_DEPLOY=false`** — в Settings → Secrets and variables → Actions → Variables (**не** как Environment Variable: условие запуска job проверяется до входа в окружение).

После сохранения секретов и успешного тестового этапа `install=false` удалить **только временный закрытый ключ на VPS**: `rm -f /root/.config/tor/tor_deploy_ed25519`. Открытый ключ в `authorized_keys` не удалять.
Для проверки first stage достаточно записи в `authorized_keys` и доступа к `/opt/tor/releases`: workflow копирует бинарник и испытывает отдельный loopback-порт **3099**, не меняя текущий сайт.

## 3. Установить systemd только после успешного stage

Перенести [tor-web.service.example](../../ops/tor-web.service.example) в `/etc/systemd/system/tor-web.service` (обязательно проверить пути) и выполнить `systemctl daemon-reload`. Не включать службу до появления `/opt/tor/current/tor-web` после первого `install=true`.

Для разрешения GitHub Actions рестартовать **только** эту службу создать через `visudo -f /etc/sudoers.d/tor-deploy` запись:

```text
tor-deploy ALL=(root) NOPASSWD: /usr/bin/systemctl restart tor-web.service, /usr/bin/systemctl stop tor-web.service
```

Проверить `visudo -cf /etc/sudoers.d/tor-deploy`. Других команд с root не разрешать.

## 4. Nginx и первый визуальный адрес

На VPS сначала посмотреть действующую конфигурацию: `nginx -T` (не отправлять содержимое ключей/секретов), `ls -l /etc/nginx/sites-enabled`, `ss -lntp`. Согласовать принадлежащий TOR домен и TLS-сертификат; затем в отдельном vhost использовать `proxy_pass http://127.0.0.1:3000;` и `X-Robots-Tag: noindex, nofollow` до беты. Только после `nginx -t` применять `systemctl reload nginx`. Не размещать TOR на прежнем домене автоматически.

## 5. Workflow

[tor-vps-staging.yml](../../.github/workflows/tor-vps-staging.yml) поддерживает `workflow_dispatch` с `install=false` (только stage) и `install=true` (активировать после stage). Изолированная сборка, архитектура и тесты проходят **на GitHub runner**, не на VPS.

После первого успешного вручную активированного релиза, проверки HTTPS в браузере, возврата на предыдущий SHA и отсутствия переполнения RAM переключить GitHub Environment Variable `TOR_AUTO_DEPLOY=true`. Тогда **только успешный main CI** `Rust S0 browser preview` запускает поставку; для PR и непроверенных SHA доступа к SSH не будет. На малом сервере дополнительная проверка **не запускает второй процесс на постоянно открытом внешнем порту**: кандидат живёт временно на loopback:3099.

Финальный 100% NEwBOO **невозможен на одном RAM-демо**: настоящая PG (здесь приёмка ресурсов после очистки) или иное постоянное транзакционное хранилище понадобится перед учётными записями, прогрессом и деньгами.

## 6. Проверка отката и защита демо (26.10.2026)

Проверены внешний доступ, пять маршрутов HTTP 200, заголовок `X-Robots-Tag: noindex, nofollow`, активная директория `c2c0429b61f06df8dd3372452f23cfd7c15ff87a` и наличие сохранённой `0a1147d5baf354f11281d143823dd008bd989d83`. Сам откат **пока не тестировался**. Доступная ОЗУ 603 МиБ.

Под root на VPS выполнить тест. Он на короткое время перезапустит `tor-web.service` **дважды**, могут возникнуть краткие 502. Nginx и DNS не меняются. Не выполнять под реальным платёжным трафиком.

```bash
curl -fsSL https://raw.githubusercontent.com/calex794/tor/main/ops/verify-tor-rollback.sh -o /root/tor-rollback-check.sh
bash -n /root/tor-rollback-check.sh &&
bash /root/tor-rollback-check.sh 0a1147d5baf354f11281d143823dd008bd989d83
```

Скрипт сверяет исходный релиз, наличие обоих бинарников и исходную готовность/HTTPS, переключает symlink на предыдущую версию и обратно, проверяет `/readyz` и `/catalog`; в обработчике выхода пытается восстановить оригинальную активную версию. Для успешной приёмки обязательны `ROLLBACK_OK`, `CURRENT_OK` и `RESTORE_OK` плюс активный исходный SHA.

**Ручное восстановление после аварии или разрыва SSH** — проверив `readlink -f /opt/tor/current`, выполнить:

```bash
ln -s /opt/tor/releases/c2c0429b61f06df8dd3372452f23cfd7c15ff87a /opt/tor/.restore-manual
mv -Tf /opt/tor/.restore-manual /opt/tor/current
systemctl restart tor-web.service
curl -fsS http://127.0.0.1:3000/readyz
```

После успешного теста можно установить **repository** Actions Variable `TOR_AUTO_DEPLOY=true`: workflow запускается только после успешного `Rust S0 browser preview` на push в main. Документальные изменения не запускают автодеплой. Для реального платного продукта необходимы отдельные интеграционные испытания и PostgreSQL.

## 7. Следующий этап: PostgreSQL 18 на 961 МиБ VPS

Новый целевой этап — [операторский PostgreSQL runbook](postgresql-18-vps.md). До положительного замера после установки не изменять демо-службу и не объявлять `readyz` проверкой БД. Старый `bokarust` содержит девять SQLx миграций; они **не** применены в TOR. Подключение приложения проводится отдельной линией I-03/I-05 с PG-тестами и проверкой восстановления.
