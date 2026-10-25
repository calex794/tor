# TOR: подготовка существующего VPS к первому визуальному запуску

**Факты 25.10.2026:** на vm15714674 старый `books_platform` удалён, `docker ps -a` пуст. `MemAvailable` вырос до 574 МиБ, swap используется 179 МиБ, свободно 13 ГБ. Системный nginx на 80/443 и SSH на 22 сохранены. Нового кода tor на VPS пока нет. Этот документ — инструкция оператору; команды здесь **не выполнялись** через ChatGPT.

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

На **своём компьютере** создать отдельный SSH deploy key с `ssh-keygen -t ed25519 -f ./tor_deploy`. Публичную часть добавить в `/home/tor-deploy/.ssh/authorized_keys` (600, владелец `tor-deploy`); приватную часть сохранить в GitHub Environment `tor-staging` как `TOR_DEPLOY_SSH_KEY`, никому её не присылать. Отдельно сверить настоящий fingerprint SSH сервера по независимому каналу и добавить соответствующую запись `known_hosts` как защищённый секрет `TOR_DEPLOY_KNOWN_HOSTS`. В GitHub Environment Variables определить `TOR_DEPLOY_HOST` (IP/имя), `TOR_DEPLOY_USER=tor-deploy`, `TOR_AUTO_DEPLOY=false`.

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
