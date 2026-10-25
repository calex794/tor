# TOR ReleaseProfile v1 — 15 дней, два режима выпуска и строгие рубежи (PREF-08)

**Цель владельца:** 15 дней: 26–30 октября — 5 суток переноса; 31 октября–6 ноября — 7 суток испытаний; 7–9 ноября — 3 суток беты. **Это целевой график, не доказательство выполнения.** Уже принято: **один monorepo TOR**, девять линий A–I, feature-first API→services→repositories, один Axum router и PostgreSQL. Не добавлять Kubernetes/микросервисы/второй repo без измеримой необходимости. Архивный G-03 / F021 / S024 (URL crawler) **заморожены** без срока.

## 1. Два разных значения слова «запуск»

**`technical_beta_limited` (Д13–Д15):**
- регистрация, сессия, public catalog/search, create/edit/publish, бесплатное чтение, библиотека/progress, безопасный reader и permissions работают на **реальной PostgreSQL**, browser;
- закрытые коммерческие, персонализированные и рекламные возможности могут отсутствовать в UI, но их публичные API **возвращают отказ**, а не фиктивное выполнение; никакая боeвая операция не двигает деньги без provider evidence;
- private S3 paid delivery, если не завершены 35 S3 подшагов/crypto/Browser E2E, остаётся выключенным, а **покупка платных глав/слоёв в публичной бете также выключена**, чтобы не продавать недоступное;
- рекомендации в режиме safe public content/metadata fallback, не выдавать это за полный перенос Python hybrid; персонификацию отключить до PG generation/opt-out/consent/evidence;
- **обязательно сообщать**, какие из F001–F061 не задействованы/какие S исключены и почему; слова «100% паритет» запрещены.

**`full_approved_scope`:** весь **утверждённый** функциональный объём TOR без замороженного F021/S024/scene/Shadow; включая две рекламы, hybrid recommendations, защищённый платный reader, ledger/refunds/payout, verified external funding, S3, social/notifications/SEO и реальные проверки на одном SHA. Готовность к такому выпуску нельзя вывести из календаря, количества задач или зелёного roadmap CI. Тотальная историческая идентичность всех NEwBOO функций **не** заявляется: исключённые по воле владельца функции остаются исключёнными.

## 2. Флаги: baseline fail-closed

Канонический машиночитаемый файл [release-flags-v1.json](release-flags-v1.json). Сочетания:

| Flag | Дефолт без доказательств | Что требуется для включения |
| --- | --- | --- |
| `paid_delivery_enabled` | false | E2E paid V1/V2/refund/90s signed S3 GET+DEK, no plaintext via Axum |
| `wallet_purchase_enabled` | false | PG atomic D purchase, saved immutable entitlement, source of funds verified |
| `boosty_funding_enabled` | false | подписанный provider event/receipt, idempotency/reconciliation; текущий probe недостаточен |
| `cash_out_enabled` | false | provider settlement/банк verified, pending reservations/holds/recovery |
| `legacy_ads_purchase_enabled` | false | fully-prepaid days, campaign wallet ledger and rate/eligibility verified |
| `chapter_ads_purchase_enabled` | false | funded package budget lock, no duplicate debit, source of funds valid |
| `chapter_ads_serving_enabled` | false | approved host/promoted eligibility, bounded planner/generation |
| `chapter_ads_billable_enabled` | false | actual 60%/1s viewable candidates, hard cap/anti-fraud/atomic reservation |
| `chapter_ads_settlement_enabled` | false | atomic posting publisher/platform, cancel/unused refund and ledger reconciliation |
| `personalized_recommendations_enabled` | false | PG generation+RIL signal provenance/consent+quality staged rollout |
| `recommendations_public_fallback_enabled` | true | только published/public metadata, live CanDiscover before response |
| `s3_uploads_enabled` | false | реальный S3 provider, CORS/KEK/crypto/GC/upload fixtures |

**Неподключаемые маршруты по умолчанию:** `/api/v1/import-jobs mode=url` → 422 `URL_IMPORT_FROZEN`. Не вводить `url_import_enabled`, т.к. владелец специально заморозил scope без срока, а не отключил флажком внутри готового продукта.

**Зависимые флаги:** `wallet_purchase_enabled` запрещён если `paid_delivery_enabled=false` для объекта, который выдаёт только private paid S3; `chapter_ads_billable_enabled` запрещён если не готовы purchase/serving/settlement и финансовые границы; `cash_out_enabled` запрещён без исходного verified cash-out provider; персонализированные rec нельзя включать без `RIL + opt-out + privacy retention`. Проверять конфигурацию на startup и fail readiness, а не надеяться на ручную последовательность включения. Для non-billable organic recommendations допустим fallback без PG personalized graph.

## 3. Календарь, акценты и передачи между линиями

| Дата/этап | Подтверждаемая сдача |
| --- | --- |
| **Д0** до старта веток | 61F/69S owners/архив, shared API/RIL/S3/финансы; независимые миграции и порты A–I, frozen catalog/config |
| **Д1: 26.10** | PG миграции с самого начала, реальная регистрация A+public book B+reader C, I веб, не S0 in-memory demo |
| **Д2: 27.10** | B версии/публикация/состав, G S3 store fixture, C grant на test-port, H WebCrypto fixture, G outbox |
| **Д3: 28.10** | D одна денежная транзакция/entitlement + S3 paid V1 path в staging, B/C/D/G/H/I вместе |
| **Д4: 29.10** | E social, F search/rec/ads algorithms на mocks, G imports files + notifications, A admin roles |
| **Д5: 30.10** | Все предметные ветки либо предъявили executable slice с tests/PR, либо статус `blocked` и флаг false; интегратор I собирает конкретный SHA |
| **Д6–Д7: 31.10–01.11** | Контролируемый корпус file import, 501 глав, две учётки, бесплатное/платное, инвалидация/доступ |
| **Д8–Д9: 02–03.11** | Паритет поведения/F/S, borrower paid V1 vs V2, Layer lifetime/30d, refunds, rec cold-start, ad caps на PG |
| **Д10–Д11: 04–05.11** | Chaos retry/network outage/PG+S3 rollback, concurrent purchase/refund/impression, money reconciles |
| **Д12: 06.11** | Один release SHA: cargo fmt/clippy/unit, PG integration, S3 + Chromium, 61F/69S release-matrix, backup restore evidence |
| **Д13–Д15: 07–09.11** | Ограниченная бета с **только** проверенными флагами, safety/watch/rollback; отдельное решение о полном открытии |

**Подход к тестам:** исполнитель завершает всю свою ветку по плану, затем запускает собственный набор тестов, исправляет пачку ошибок и повторяет; на каждом локальном микрошаге только cargo fmt/check/дешёвый module contract. Обязательные **пакеты**: при изменении money/ACL/S3/crypto/outbox/moderation запускается общая cross-domain security suite независимо от selective paths. Окончательная проверка полного билда/PG/S3/browser на **конечном SHA**, а не на предыдущем зелёном коммите. Правила ветки main enforced только после GitHub Rulesets (Actions alone не защита).

## 4. Release gates как фактические испытания

| Gate | Команды и артефакты | Блокировка |
| --- | --- | --- |
| REL-01 Workspace | `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` | compile/test failure |
| REL-02 Architecture | `python3 tools/check_roadmap.py`, `python3 tools/check_contracts.py`, `python3 -m unittest discover -s tests/architecture -v`, `python3 tools/check_feature_layout.py` | lost scenario, core contract or non-feature split |
| REL-03 PG | fresh PG migration up + backward-compatible rollback strategy; 2 clients race/restore, `pg_dump` fixture restore on isolated DB | missing FK/uniqueness/ledger/idempotency/GRANT data |
| REL-04 Paid S3 | HTTPS/CORS/anonymous private deny/WebCrypto vectors/key rotation/KEK backup/signed GET, Network shows S3→browser | absence of real S3, S3 plaintext, paid grant leaked |
| REL-05 Product ACL | Chapter V1/V2, private/blocked/expired/revoked/refunded, creator withdrawal, Layer 30d/lifetime, cross Actor network/cache | paid without entitlement, paid+revoked no disposition |
| REL-06 Ad/Commerce | ads replay/escrow/cumulative minor/hard impressions cap/settlement/payout/refund, real PG; if not verified flags OFF | incorrect money or live advertiser charging |
| REL-07 Recs/Events | Python source golden/reason/weights, 0 users fallback, RIL RoutePassport/verified completion, 5 surfaces | leaked private/18+, forged data, fabricated uplift |
| REL-08 Load/Recovery | VPS measured memory+CPU+disk, 2 workers lease, network flaps, DB reconnect, S3 outage, restore KEK+V1, log secrets scan | OOM/hanging checkout/lost encryption key |
| REL-09 Browser | real Chromium e2e guest/reader/author/admin with PG/S3, 501 chapters, responsive, direct denied URLs | no browser proof / S0 RAM facade |
| REL-10 Public availability | healthz/readiness, stable deploy slot/version and tested rollback, actual TLS/backup/telemetry dashboards | deploy green but site unusable |

**Защита от ложного «зелёного»:** внешний платёжный провайдер не подключён → ни Boosty, ни cash-out не готов, независимо от внутренней mock проводки. Если paid reader не готов, покупка запрещена. Если ads выключены, S042/S043 имеют статус blocked/off, не `done`. Если F024 заморожен, это исключение, а не выполненная функция.

## 5. Протокол результата каждого исполнителя

```text
lane: D
task: D-06 / FIN-B05
scope: crates/commerce/** + contracts/finance
branch/PR: URL
commit_sha: full 40 hex
API: POST /api/v1/admin/refunds/{id}/decide
PG migrations: paths and transaction isolation
test_results: exact command + passed/failed + PG connection mode
browser_evidence: Chromium run URL or explicit NOT_RUN
dependencies: B chapter_version; C first grant barrier; G outbox
feature_flags: wallet_purchase_enabled=false if not fully verified
status: drafted | code | compiled | PG-tested | browser-tested | released | blocked
remaining: explicit edge case + owner
```

Состояния S/F хранить **в существующей базе Notion**, никакой второй авторитетной таблицы. Прогресс=доказанные категории, не % по числу markdown-файлов. PR с documentation accepted не меняет `status=released`.

## 6. Блокеры, которые нельзя скрывать

1. Нет настоящего внешнего подтверждения оплаты/выплаты — публичные topups и cash-out **не включать**; пользователь может тестировать по ограниченным фикстурам staging, но не за реальные деньги.
2. Не предоставлены реальный S3 endpoint/bucket/IAM/CORS и независимый KEK backup — paid S3 capability **not ready**.
3. Не подтверждена конкретная policy сроков ordinary refund/коммерческих тарифов/долей — брать **immutable configured offer**; пустой offer не превращать в денежную операцию.
4. Нельзя обещать отсутствие копирования текста после client WebCrypto: signed GET bearer/DEK можно сохранить, защита ограничена TTL/доступом и не является DRM.
5. При нулевой аудитории и отсутствии реальных read событий улучшение качества персонализации не доказать; rollout включать только по техническим метрикам и после накопления допустимых событий.
6. В TOR остаются архивные возможности NEwBOO. «Полный продукт» означает только **утверждённый scope**, не переписывание исторически исключённых функций.
