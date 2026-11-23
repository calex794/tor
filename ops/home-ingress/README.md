# TOR — будущий внешний шлюз, только образцы

Решение: Rust/TOR и PostgreSQL 18 работают на домашнем сервере. VPS принимает HTTPS, отправляет динамические запросы через отдельный WireGuard. Подходящие S3-объекты браузер получает напрямую.

**Никакой установки:** файлы в этой папке не копируются, не подключаются автоматически и не меняют работающие службы. Старое развёртывание TOR на VPS остаётся отдельной исторической схемой, а не инструкцией для нового шлюза.

## Перед использованием

1. Проверить, что подсеть примера `10.88.0.0/30` и UDP-порт `51820` не конфликтуют с домашней сетью, AmneziaVPN/AWG и существующими службами.
2. Создать ключи отдельно на VPS и дома. Не помещать реальные ключи в GitHub. Настройки WireGuard должны храниться под `root` с правами `0600`.
3. На VPS только 80/443 TCP и проверенный UDP-порт; SSH ограничить доверенным доступом. Не открывать наружу 5432/3000/18081.
4. Дома TOR привязан к `127.0.0.1:3000`, PostgreSQL — к `localhost:5432`, а принимающий Nginx только к `10.88.0.2:18081`. Дополнительно брандмауэр разрешает 18081 только от `10.88.0.1` через WireGuard. На домашнем роутере нет входящей переадресации портов приложения.
5. Внешний TLS-сертификат для `pandaolate.ru` необходимо получить и настроить до включения шаблона HTTPS. Пример предполагает уже работающие пути Let's Encrypt и не выдаёт сертификат.
6. Внешний Nginx перезаписывает подставленные клиентом Host, X-Forwarded-* и Forwarded. Домашний приёмник пропускает заголовки только от доверенного внешнего VPS; за дополнительным CDN нужен отдельный договор о реальном IP.
7. Тело HTTP до 34 МиБ ограничено на обоих шлюзах; приложение продолжает применять собственный предел импорта 32 МиБ и прочие проверки. Приватные ответы и короткие S3-ссылки нельзя кэшировать на VPS.
8. Перед переключением проверить `nginx -t`, TLS, доступность платных глав, авторизацию, скрытие диагностических маршрутов, обрыв/восстановление WireGuard, отсутствие обхода домашнего HTTP/PG. Сохранить старую рабочую конфигурацию для отката.
9. Замеры GitHub Actions не эквивалентны реальному VPS с 1 виртуальным ядром и 1 ГБ памяти. Тариф выбирается окончательно после испытания конкретного узла.

Файлы: `vps-site.nginx.conf.example`, `home-site.nginx.conf.example`, `vps-wg0.conf.example`, `home-wg0.conf.example`.

Связанные документы: [лабораторный замер](../../docs/infra/vps-ingress-sizing.md), [Notion TOR-HOME-01](https://app.notion.com/p/40832a84beec8103b8d3c43170472381).

## Предварительная проверка сети — без доступа к серверам

`tools/home_network_preflight.py` читает **только вручную подготовленный обезличенный JSON-снимок**. Не вызывает SSH, не открывает соединений, не читает конфигурации узлов, не получает ключи и не меняет настройки. Успешный результат означает согласованность *предполагаемой* топологии, а не проверку фактического домашнего компьютера.

Поля снимка: `schema=tor.home_ingress_preflight.v1`, `tunnel_network` (пример `10.88.0.0/30`), `edge_tunnel_ip` и `home_tunnel_ip`, `other_networks` (список объектов `role,cidr` с возможными `home_lan`, `vps_lan`, `vpn`, `docker`, `other`), `edge_wg_udp_port`, `edge_busy_udp_ports`, `edge_tcp_service_owners` (карта `80` и `443` → `nginx`), `edge_peer_allowed_ips`, `home_peer_allowed_ips`, `home_initiates_tunnel`, `home_default_route_through_tunnel`, `home_router_to_tor_port_forwards` и `home_service_bind_candidates` (ключи `tor-web`, `postgresql`, `home-nginx`; значения — списки строк привязки).

Команда для **локального учебного снимка**:

```sh
python3 tools/home_network_preflight.py --input path/to/sanitized-snapshot.json
python3 -m unittest tests.architecture.test_home_network_preflight
```

Коды возврата: 0 — предложенная схема непротиворечива, 1 — выявлены препятствия, 2 — неполный или неверный снимок. В отчёте сохраняются только имена проверок и результат; адреса из входного файла не копируются. Не вносите реальный публичный IP, ключи VPN, пароли, токены или сведения об иных серверах в GitHub. Текущая версия намеренно требует адреса и порт примера, изменение допустимо лишь после отдельного решения и тестов. Ручной осмотр реального брандмауэра, маршрутизатора и TLS остаётся обязательным до запуска.

Эта проверка никак не подключается к боевому VPS; никакой выкладки из GitHub Actions.


## AB-EDGE: Nginx protection and 15-minute security snapshot

This is **an offline example for the future public gateway**, not the current VPS
configuration and not a deployment. It does not modify running infrastructure.

The example:
- Drops well-known WordPress/PHP scanner paths with inexpensive Nginx 444
  responses; normal unknown paths, legal archives and crawlers are not banned.
- Defines a generic 6 requests/second per direct peer with burst 80 and a
  separate 1 POST/second, burst 8 for login and registration.
  Both rate limits are initially in **dry-run**: would-be rejects are logged
  as REJECTED_DRY_RUN; actual requests are not limited by Nginx yet.
- Reuses a bounded pool of 16 upstream keepalive connections across WireGuard
  to avoid local TCP source-port exhaustion from repeated short requests.
- Uses ONLY the public-edge $binary_remote_addr as key. Never trust headers
  supplied by the browser. If adding a CDN, re-review the real-IP chain first.
- Writes a restricted security log with timestamp, direct IP, method, status,
  bounded route family, limit decision and latency. No URL, query, User-Agent,
  referrer, cookies, book IDs or tokens. IP still counts as personal data.
  Install the included logrotate example (seven days, 0640) before enabling it.
- Provides tools/security_log_report.py for a bounded read-only 15-minute
  summary. Default source labels are **coarsened**, not anonymous: /24 for
  IPv4 or /64 for IPv6. These networks may still identify people; exact IP
  is only shown with --show-full-ip. Signals are not unique bots or attack proof.

Before any production rollout: obtain a READ-ONLY inspection of the active
nginx -T (without copying secrets/IPs), its owner of 443, where its access logs
live, whether a CDN/proxy is in front, and the current runtime environment.
Set TOR_AUTH_TRUST_NGINX_REAL_IP=1 ONLY when the loopback Nginx overwrites
X-Real-IP from a verified public gateway and direct app bypass is impossible.
Otherwise per-IP auth throttling deliberately disables itself, but per-account
PostgreSQL throttling remains enforced. Never flip that flag blindly.

Run nginx -t and isolated tests; check legitimate reading, login/registration,
sitemaps, long sessions, shared NAT, signed S3 URL requests and false positives.
Then consider limit_req_dry_run off with independent approval and rollback.
This PR does NOT activate the new Nginx, CAPTCHA, automatic IP bans, or deploy.
The existing admin traffic PR #1191 remains a separate process-counter feature.

Offline tests: python3 -m unittest tests.architecture.test_security_log_report
Disposable edge tests: tests/load/home_ingress_examples_ci.sh (CI only).
