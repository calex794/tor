# DEPLOY-01 / I-11 — единый подписанный комплект TOR

Цель: один проверяемый выпуск приложения и управляемых файлов инфраструктуры,
без автоматической выкладки после merge и без расширения sudoers.

## Формат v2

Три бинарника: tor-web, tor-migrate, tor-trace-store.
Восемь файлов аварийной страницы и Nginx из FALLBACK-01–03.
Один узкий управляемый systemd drop-in (DEPLOY-04).
Только фиксированные имена из tools/release_bundle_v2.py, без tar-архива,
произвольных путей, ссылок, дополнительных файлов и скрытых данных.
fallback-config.example.json — шаблон, а не пользовательская конфигурация.
Ключи TLS/SSH/S3, PG/WAL, production env, Telegram и снимки каталога
не входят в пакет. Основной unit и операторские drop-ins не заменяются.

Манифест tor-release-manifest-v2 содержит версию, полный SHA выбранного
коммита и SHA-256 каждого файла в фиксированном порядке. Отдельная
SSH-подпись использует identity tor-release и namespace tor-release-v2
(подпись v1 не может авторизовать пакет v2).

## Исполняемый контракт

После принятия FALLBACK-01–03, в изолированном CI:

```sh
python3 tools/release_bundle_v2.py build --repo . \
  --binaries target/release --directory "$RUNNER_TEMP/tor-bundle" --sha "$TARGET_SHA"
ssh-keygen -Y sign -f "$SIGNING_KEY" -n tor-release-v2 \
  "$RUNNER_TEMP/tor-bundle/tor-release-manifest-v2"
python3 tools/release_bundle_v2.py verify --sha "$TARGET_SHA" \
  --signers "$ALLOWED_SIGNERS" --directory "$RUNNER_TEMP/tor-bundle"
```

На следующем этапе root-owned executor должен копировать содержимое inbox
в собственный временный каталог, затем проверять подпись и хеши именно
копии (stage); только проверенную копию разрешено использовать.
Директория назначения новая, находится в доверенном каталоге,
принадлежащем вызывающему пользователю, без прав записи group/other.
Вызывающая сторона обязана держать уже существующий release lock.
stage не меняет активный релиз, Nginx, systemd или базы, не получает sudo
самостоятельно. Проверка verify предназначена для доверенной копии.

## Совместимость и приёмка

Старый production workflow и root executor принимают только v1.
Они намеренно не изменены данным PR: нельзя менять формат перед
готовностью доверенного исполнителя и испытаниями отказов.
Следующий DEPLOY-02–03: установить фиксированный vhost/include-контракт,
проверить candidate nginx -t, подключить статические файлы, сохранять
операторские настройки и проверить откат. Затем согласованно включить v2
в существующий workflow и обновить доверенный root helper отдельным
операторским bootstrap. Никаких новых прав GitHub.

Тесты выполняются существующим Repository rules через discovery:
python3 -m unittest discover -s tests/architecture -p 'test_release_bundle_v2.py' -v

В текущем PR это проверка формата с тестовыми файлами и настоящими
одноразовыми Ed25519-подписями. Сборка полного комплекта из реального
FALLBACK и испытание root activation / Nginx / rollback остаются открыты.
PostgreSQL не изменяется; миграции и политики доступа прежние.
