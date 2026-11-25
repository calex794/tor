# DEPLOY-02b / I-11 — реальное испытание управляемого NGINX

Один изолированный Linux network namespace с loopback only, отдельным
master NGINX, временным сертификатом Pandaolate и одноразовым Ed25519
signer. Используются файлы FALLBACK из текущего checkout и production
Python helpers; три бинарника в комплекте — инертные тестовые файлы,
не Rust-сборка. Никаких production host, credentials, systemd reload,
PostgreSQL, S3 или изменения системного nginx.conf.

Проверка выполняет настоящий build/sign/stage, первичную установку при
неработающем backend, реальный nginx -t и reload, проверку HTTPS маркера
и хеша статического файла. Два исправных подписанных комплекта отличаются
содержимым JS. Два ошибочных комплекта имеют корректную подпись, но один
не проходит nginx -t, другой возвращает 403 для game.js после reload.
Проверяется восстановление исходного vhost при первой установке и
возврат предыдущей версии при обновлении. Проверяются HSTS, HTTP 502,
изолированные API/POST/admin ответы, настройки Telegram, запрет доступа
к internal resource, восстановление upstream и неизменный master PID.

Для теста override заменяет только выбор изолированного nginx config
и reload на сигнал собственному master; реальные production probe и
adoption snapshot используются без отключения проверки TLS. Curl
доверяет только одноразовому тестовому сертификату через CURL_CA_BUNDLE.
Rollback первой установки повторяет проверку HTTPS ограниченное время,
так как nginx reload асинхронен.

Workflow tor-fallback-edge.yml запускает стенд после обычных FALLBACK
HTTP/Chromium испытаний. Только pull_request / workflow_dispatch, read-only.
Запуск стенда от root допустим только внутри отдельного network namespace,
проверяется отказ вне namespace. Журнал managed-edge-evidence.
Не доказательство готовности реального VPS или комплексного Rust rollback:
следующие задачи — trusted bootstrap, systemd, журнал и совместный release.

Regression: reload now waits for the same Linux Nginx master (PID and
process start time), a distinct active worker generation and three
consecutive observations without old active workers. Draining workers may
finish established connections. The actual TLS marker/hash and rollback
snapshot checks run after this fence, not immediately after signal delivery.
