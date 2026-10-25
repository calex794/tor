# TOR — две рекламные системы: подробная очередь реализации и допуск к продаже

**Статус:** техническая детализация F-10/F-11 + D-11 + G-06/G-07 + H, составлена по [аудиту Advertising / Chapter Ads](https://app.notion.com/p/3cd32a84beec81b980f0c276d8b4cb0d) и [MONEY-04](https://app.notion.com/p/40432a84beec81f193bccca87a4ef4e0), NEwBOO F031/F032, сценарии S042/S043. **В TOR этого ещё нет.** Два исходных продукта нельзя сводить в одну таблицу или в один `POST /api/v1/ads/campaigns`.

Старый Python имеет: [legacy daily campaigns](https://github.com/HeisLuka/newboo/tree/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/advertising), [Chapter Ads](https://github.com/HeisLuka/newboo/tree/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/advertising/chapter_ads). Legacy покупает **дни и размещения**, Chapter Ads — **пакет оплачиваемых показов**. Это **внутренние** рекламные продукты сайта; отсутствие сторонней рекламной сети само по себе не является причиной не реализовывать собственный рекламный движок. Но оплата и billable serving по умолчанию **OFF**, пока не доказан внутренний финансовый контур.

## 1. Два независимых продукта, общие ограничения

| Инвариант | Legacy campaigns | Chapter Ads |
| --- | --- | --- |
| Продаём | `placement_type + диапазон дат`, целое число дней | `package_id`, `target_impressions`, `budget_total_micros` |
| Единица денег | RUB minor units через тип D; snapshot locked rate | `RUB micros` внутри ad escrow; wallet D в копейках |
| Варианты | **только fully-prepaid**, `prepaid_days == total_days`; credit/daily refill не переносить | весь пакет предоплачен, расход — после фактического billable impression |
| Реальный расход | posted purchase / подтверждённое обслуживание рекламных дней по policy | атомарно зарезервированный показ, затем settlement |
| Исходная архитектура | `advertising/services/campaign_purchase_service.py` и daily billing | `chapter_ads/services/serving_service.py`, impression recorder, settlement |
| Включение | `legacy_purchase_enabled` / `legacy_delivery_enabled` | отдельные `chapter_ads_purchase,planner,serving,billable,settlement_enabled` |
| Общие проверки | actor=владелец, книга разрешена, статусы, бюджет, stable intent, no paid-content leak | + host/promoted пара, исключение self-ad и publisher owner, viewability/antibot |

Любая ad карточка — только публичные метаданные книги; роль рекламодателя/оплата **не** дают право видеть hidden/paid chapter. Использовать типизированный порт `CanDiscoverBook` из F/B/A, а не отдельные ad-ACL правки.

## 2. Исполняемая очередь без самостоятельного проектирования

| ID | Владелец/файл результата | Конкретная операция и готовый исход | Тест |
| --- | --- | --- | --- |
| ADS-01 | I: `architecture/modules.toml` + `contracts/src/ads.rs` | Зарегистрировать `tor-ads` за F. Типы `AdCampaignId,LegacyPlacement,ChapterAdPackage,MoneyMicros,AdImpressionId,AdSettlementId`, версии ответа/отказов и `AdFundingPort`/`AdBookEligibility`. | Сборка контрактов и отсутствие прямого Cargo ads→commerce |
| ADS-02 | F: `crates/ads/src/domain/legacy.rs` | Создавать legacy период, immutable price/rate/start/end, проверить owner+book, проверка пересечений placement и `fully_prepaid`. | Два перекрывающихся периода 409; попытка купить 3/7 кредитных дней 422 |
| ADS-03 | D: `commerce/services/ad_funding.rs` + миграция через I | Stable `Idempotency-Key`; один verified wallet debit/escrow posting и сохранённая purchase-команда; pending/failed replay с другим payload 409, correct posted evidence before activation. | 2 HTTP submit/double-click + crash после COMMIT → 1 debit, 1 funded campaign |
| ADS-04 | F: `ads/services/legacy_serving.rs` | Подключить размещения по дням, pacing/anti_clump/session caps, возраст/видимость, deterministic sorting, выключение при окончании периода. | Серия одинаковых запросов не перезаполняет слот; blocked book 0 cards, законченные дни 0 delivery |
| ADS-05 | F: `ads/services/chapter_packages.rs` | Создать immutable пакет с `target_impressions>0`, `price_micros>0`, `currency=RUB`, `share_bps` 0–10000; stage package purchase по snapshot. | Изменённая цена после формы →409 без денег; пустой пакет не активен |
| ADS-06 | D: `commerce/repositories/ad_posting.rs`; F: `ads/services/chapter_purchase.rs` | В **одной критической PG-транзакции через узкий posting port** зафиксировать campaign pending→posted и escrow budget lock, advertiser debit+ledger; F campaign-row mutation — задокументированное исключение cross-domain атомарности, интегратор I владеет миграцией/процедурой. | Завершённый commit содержит все факты; обрыв до commit оставляет none; replay не создаёт второй hold |
| ADS-07 | F: `ads/services/chapter_planner.rs` | Долговечное generation с bounded host buckets и слотами; eligibility host/promoted в начале и перед serving, self-ad и advertiser/publisher identities исключены; активировать новое поколение одним pointer. | Новый generation сбойный → старый остается active; public reader request без полного сканирования assignments |
| ADS-08 | H/F: `web/assets/chapter-ads.js`, `ads/services/viewable.rs` | Наблюдать реальную видимость рекламной карточки: **≥60% viewport ≥1 сек** по подтверждённому клиентскому событию, server-side replay/generation check и антибот. Не считать DOM-render показом. | 59%/0.9s → not billable, 60%/1s+ → candidate; forged event/replay rejected |
| ADS-09 | F: `ads/repositories/impression_reservations.rs` | При accepted impression сделать **в одном PG commit**: event dedup, атомарный `reserved_impressions +1`, `reserved_budget_micros + rate` и bounded stats. `target/budget` — hard cap на момент приёма, не на следующий час. | 100-показный cap и 20 конкурентных запросов на 98/100 → принять ровно 2; ни одной сверхоплаты |
| ADS-10 | F: `ads/repositories/frequency_buckets.rs` | Перенести дневные/hour/session лимиты из process-local памяти в TTL/PG counters keyed по псевдониму, не raw IP; индексы `subject,campaign,day/hour`. | После рестарта/2 воркеров лимиты не обнуляются; anonymous spray заблокирован |
| ADS-11 | D: `commerce/services/ad_settlement.rs` | Hourly close: по reservation + детерминированный settlement ID, atomic escrow debit → platform earnings → publisher hold (исторический `clear_at+72h`) → settled marker, idempotent. Для одного commit F+ D использует согласованный D posting transaction port. | Crash before/after settlement, два worker, повторный hour: деньги распределены ровно один раз |
| ADS-12 | D: `commerce/services/ad_refund.rs` | Отмена: остановить новые reservations; завершить подтверждённые, unused escrow вернуть исходному advertiser wallet; state `admin_cancelled/refunded` с audit. | Cancellation/retry не выводят баланс в минус; refunded≤funded-settled |
| ADS-13 | D: `commerce/services/ad_publisher_balance.rs` | `pending→available` по clear_at, вывести в creator wallet через **атомарный** typed transfer, без чтения всего ad-ledger. Сверка общей суммы publisher+platform+refund+escrow. | Частичный сбой/replay не создаёт double withdrawal |
| ADS-14 | F: `ads/services/attribution.rs` | Отдельно от оплаты impression: signed campaign context, настоящий клик/open/read/library/purchase через canonical handlers; CTR/conversion измеряются только подтверждёнными действиями, без клиентской подделки. | Поддельный click/conversion не считается; не менять CPM/billable на CPC |
| ADS-15 | F/H: `ads/repositories/dashboard.rs`, `web/pages/ads` | Личный кабинет advertiser+publisher: стоимость/заказы, показ/бюджет/reservations, earnings/payout/ошибки; SQL cursor по `campaign_id,publisher_id,date_hour`. Без global stats scan и lifetime SUM при каждом GET. | 100k stats rows, запрос page≤100, чужая статистика 403, bounded query plan |
| ADS-16 | I+F+D+G+H | Сквозная реальная PG+Chromium при ошибках: выбрать книгу → купить кампанию → назначить слот → получить viewable impression → settlement → publisher balance → cancel unused budget → отчёт. | Проверка сумм, гарантированный refund, concurrency/lease/replay, разрыв сети и ACL на одном SHA; только после этого `*_enabled=true` |

**Таблицы (проект миграций I):** `ad_packages`, `ad_legacy_campaigns`, `ad_chapter_campaigns`, `ad_delivery_generations`, `ad_delivery_assignments`, `ad_impression_reservations`, `ad_impression_dedup`, `ad_frequency_buckets`, `ad_delivery_hourly_stats`, `ad_settlements`, `ad_campaign_attribution`; денежные счета/журнал/escrow/holds принадлежат D. В F обязательно индексы активного generation + host bucket + campaign/day, а не full table scan.

**Граница единиц:** Python Chapter Ads использует integer `micros`; обычный TOR wallet — integer **копейки**. Запрещено прямо складывать их, округлять через `f64` или терять остатки. Для нового TOR установить typed преобразование `1 копейка = 10 000 RUB micros`; escrow/reservation/settlement вести в micros и только при переводе в кошелёк переводить по подтверждённой политике округления, оставляя учтённый micro-remainder. Точная политика остатка при завершённом пакете (и распределения не делящейся нацело цены по N показам) — **одно оставшееся финансовое продуктово-учётное решение**, нельзя скрывать в integer truncation из Python.

**API нужно добавить раздельно** (сохранить HTML-совместимость после сверки):
- legacy: `POST /api/v1/ads/daily-campaigns`, `GET /api/v1/me/ads/daily-campaigns`, `POST /api/v1/ads/daily-campaigns/{id}/cancel`;
- chapter ads: `GET /api/v1/ads/chapter/packages`, `POST /api/v1/ads/chapter/campaigns`, `GET /api/v1/ads/chapter/campaigns/{id}`, `POST /api/v1/ads/chapter/impressions`, `POST /api/v1/ads/chapter/campaigns/{id}/cancel`, `GET /api/v1/me/chapter-ads/publisher-ledger`;
- все write требуют Actor, CSRF/Origin для cookie, стабильный Idempotency-Key (если создаёт/перемещает деньги), bounded body, access check, typed error. `/api/v1/ads/campaigns` можно сохранить как redirect/alias только после проверки старого поведения, **не** смешивать day package/impressions в одном request DTO.

## 3. Условия рекламного запуска

**Техническая готовность**: ADS-01…16, PG fake/real tests, load tests cap, 72h hold simulation, бухгалтерская сверка по каждой кампании, безопасное выключение отдельных flags, зафиксированная версия API.

**Продуктовое решение**: включать ли обе рекламные модели в первые 15 дней? Текущая общая дорожная карта обещает функциональный перенос, но запрещает boevoy settlement до проверки. Если оставить обе выключенными, **полный паритет рекламы не достигнут**; это допустимо только для **ограниченной беты без рекламы**, не для публичного рекламного бизнеса.

**Нет внешнего ad-provider gate:** это собственные размещения книги. Реальный Boosty topup — независимый внешний источник денег; можно проверить ads на trusted test fixture, но **не продавать рекламные пакеты за фиктивные средства в публичной версии**. Деньги и рекламу не считать завершёнными на основании UI/таблиц.
