# TOR Finance/Ads v1 — точная сумма, расчёт показов и балансировка (FIN-01)

**Статус:** выбранная техническая модель для Rust/PostgreSQL; внешние реальные деньги/коммерческий тариф не утверждаются этим документом. Зависимости: [две рекламные модели](../roadmap/ads-execution-contract.md), [общий ledger](../roadmap/data-and-events.md), [NEwBOO MONEY-04](https://app.notion.com/p/40432a84beec81f193bccca87a4ef4e0) и [проверка Chapter Ads](https://app.notion.com/p/3cd32a84beec81b980f0c276d8b4cb0d). Владелец D — финансовые записи, F — показ и рекламная кампания, I — типы/миграции, G — повтор/доставка.

## 1. Конкретные единицы

- Валюта первой версии — **только RUB**. Внешний wallet, книга, выплата, refund и ledger используют **целое количество копеек** `MoneyMinor(i64)`; отрицательное значение разрешено только *delta проводки*, не цену или положительный баланс.
- Python Chapter Ads исходно хранит `price_micros` с правилом `price_micros % target_impressions == 0`. Математика: **1 копейка = 10 000 RUB micros**. На TOR границе `Micros(i128)` сериализуется только строкой/целым, конвертация в `MoneyMinor` только если `micros % 10_000 == 0`. Цену пакета с дробной копейкой **не округлять и не продавать**: `422 AD_PRICE_NOT_MINOR_ALIGNED`, нужен новый явно опубликованный прайс.
- **Выбранное отличие TOR от старого поштучного micros-тарифа:** reserve и реальные деньги по показам всегда ведутся в **целых копейках с накопительным распределением**, чтобы не создавать невыводимых микроостатков между кошельком и escrow. Исходные `gross_rate_micros` служат проверяемой исторической справкой, не отдельным активным кошельком. Для неизменной продажи сохраняются `price_micros_at_offer` и `pricing_algorithm="cumulative_minor_v1"`.
- Знак и переполнение: вычисление `budget_minor * delivered_count`, `gross * share_bps` вести в `i128` с проверкой преобразования в `i64`. `f32/f64` и `Decimal` из HTML не являются источником истины денег. UI допускает только десятичные целые minor или явный RUB format в копейках с точным парсером; дроби ниже 1 копейки отвергнуть.

## 2. Без округлённых задолженностей: пакет N показов за B копеек

Пакет замораживает `B = budget_total_minor > 0`, `N = target_impressions > 0`, `pub_bps ∈ [0,10000]`, `platform_bps = 10000-pub_bps`, `currency=RUB`. После каждой атомарной принятой операции `k = delivered_impressions`:

```text
C(k)       = floor(B * k / N)                 # накопленный оплаченный расход, коп.
gross(k)   = C(k) - C(k-1)                    # стоимость k-го подтверждённого показа
P(k)       = floor(C(k) * pub_bps / 10000)    # накопленный доход площадки размещения
publisher_delta(k) = P(k) - P(k-1)
platform_delta(k)  = gross(k) - publisher_delta(k)
refund(k)  = B - C(k)                         # ровно неиспользованный бюджет
invariant: C(k) + refund(k) = B
invariant: publisher_total(k) + platform_total(k) = C(k)
```

**Математический пример (fixture FIN-A01):** B=101 коп., N=4, pub_bps=7000 → gross по 4 показам **25,25,25,26**, cumulative C=25/50/75/101; publisher cumulative = 17/35/52/70, delta=17/18/17/18; platform cumulative=8/15/23/31. Отмена после k=2 → 51 коп. возврата, publisher earned=35 коп, platform=15 коп. Все проводки целые; не использовать `float` или `price/target` округлённое заранее.

Если B<N, некоторые `gross(k)=0`, но показ может считаться подтверждённым и не создаёт фиктивный денежный микродолг; при k=N зарезервировано ровно B. Политика минимальной коммерческой цены на пакет — **отдельный каталог тарифов**, не встроенное произвольное ограничение счета. Нарушенные/старые micro-tiers до явной перенормировки **sale disabled**.

## 3. Единые транзакционные границы в PostgreSQL

**Покупка:** `(actor,idempotency_key,canonical_request_hash)` уникальны, цена/права/campaign offer immutable. Один SERIALIZABLE transaction/row lock: account capacity ≥B → debit advertiser wallet B → credit escrow B → campaign funded/posted, frozen offer/price/owner → `AdCampaignPurchased` in outbox. При ошибке — **0 эффектов**. Переповтор идентичного HTTP намерения возвращает исходные campaign_id/receipt; другой payload → 409 даже если исходный pending/failed.

**Подтверждённый показ:** валидный grant/event + ≥60% и ≥1 sec + проверка live book ACL + anti-fraud/дедуп + unique event → `SELECT ... FOR UPDATE` campaign → отказ при `k=N`, blocked/cancelled или exceeded; `k→k+1`, `C(k)-C(k-1)` резервировать из escrow, статистика и дедуп в **одном commit**. Сценарий 20 конкурирующих запросов при 98/100 должен принять только два.

**Settlement:** только уже зарезервированные подтверждённые `gross(k)`, один `settlement_id` → escrow reserved → publisher `immature` и platform earned → mark settled. Фактическая финансовая проводка, campaign tally и marker — **одна БД транзакция**. Ранее settled event не даёт второе начисление. Считать publisher/платформу от накопительной суммы **до k**, а не округлять каждый чек заново; поздняя смена share не переписывает старую запись.

**Отмена/возврат:** сначала остановить новые reservations (статус и lock); завершить подтверждённые уже pending settlement; в **одном trusted posting** возвратить `B-C(k)` коп. в исходный advertiser wallet и закрыть escrow; сохранить `refund_id` и immutable postings. Повтор cancel/refund не меняет баланс. До drain резервов операция может иметь `cancelling`, но не объявляется `cancelled/refunded` без факта posting.

**Оплата издателю:** подтверждённые доли помещаются в immature hold, release по сохранённому `clear_at` (historical Chapter Ads +72h; срок — конфигурация кампании при покупке), затем становятся `withdrawable` по policy происхождения денег. Payout-capacity: mature - outstanding holds - active reservations - debt. Transfer creator wallet — отдельный атомарный D posting; ни F, ни worker не меняют wallet balance вручную.

## 4. Рекомендуемый DDL и authority владельцев

Пример расширения схемы, которую создаёт I (столбцы согласовать с реально добавленными D миграциями **до** их исполнения; не создавать вторую универсальную бухгалтерию):

```sql
CREATE TABLE ad_campaign_funding (
    campaign_id TEXT PRIMARY KEY,
    advertiser_id BIGINT NOT NULL,
    currency CHAR(3) NOT NULL CHECK (currency='RUB'),
    budget_total_minor BIGINT NOT NULL CHECK (budget_total_minor>0),
    target_impressions BIGINT NOT NULL CHECK (target_impressions>0),
    publisher_share_bps INTEGER NOT NULL CHECK (publisher_share_bps BETWEEN 0 AND 10000),
    pricing_algorithm TEXT NOT NULL CHECK (pricing_algorithm='cumulative_minor_v1'),
    delivered_impressions BIGINT NOT NULL DEFAULT 0 CHECK (delivered_impressions>=0),
    reserved_minor BIGINT NOT NULL DEFAULT 0 CHECK (reserved_minor>=0),
    settled_minor BIGINT NOT NULL DEFAULT 0 CHECK (settled_minor>=0),
    refunded_minor BIGINT NOT NULL DEFAULT 0 CHECK (refunded_minor>=0),
    funding_ledger_entry_id BIGINT NOT NULL UNIQUE,
    offer_hash BYTEA NOT NULL,
    status TEXT NOT NULL,
    revision BIGINT NOT NULL DEFAULT 0,
    CHECK (settled_minor<=reserved_minor),
    CHECK (reserved_minor+refunded_minor<=budget_total_minor),
    CHECK (delivered_impressions<=target_impressions)
);
CREATE TABLE ad_billable_impressions (
    event_id TEXT PRIMARY KEY,
    campaign_id TEXT NOT NULL REFERENCES ad_campaign_funding(campaign_id),
    ordinal BIGINT NOT NULL CHECK (ordinal>0),
    gross_minor BIGINT NOT NULL CHECK (gross_minor>=0),
    reserved_at TIMESTAMPTZ NOT NULL,
    settled_at TIMESTAMPTZ,
    UNIQUE(campaign_id,ordinal)
);
```

Money ledger всегда **append-only**, одна movement identity и `(transfer_id,account_id)` уникальна; debit+credit должны балансироваться в рамках одного `transaction_id`. В `ad_campaign_funding` нельзя полагаться только на `CHECK` для ограничения funded balance/phantom double-spend: нужен row lock/serializable + unique idempotency.

## 5. Реальные приёмочные тесты и обязанности

| Проверка | Прогон | Условие |
| --- | --- | --- |
| FIN-A01 | Rust pure unit | B101/N4/share70%, последовательность и 51-копеечный refund |
| FIN-A02 | property-based | Для B=1..10_000,N=1..500,bps 0..10000 C(N)=B; pub+platform=C; refund+C=B; monotonic; no overflow |
| FIN-A03 | PostgreSQL 20 competing requests | 98/100 → две успешных резервирования, 18 отказов; ledger однажды на event |
| FIN-A04 | HTTP same/new idempotency payload | one debit+escrow for double-click; changed payload 409 |
| FIN-A05 | crash at funding/reserve/settle/refund boundaries | после рестарта по ключу один эффект, balances reconcile |
| FIN-A06 | two publisher withdrawal workers | один transfer на maturity id, no duplicate payout |
| FIN-A07 | migration old micro-tier | `price_micros % 10000 != 0` denied until repriced; не скрыто округлять |
| FIN-A08 | pending settlement + cancel | сначала settle accepted, затем только unused refund; ничего не потеряно |

**Гейт:** Пока все реальные PG/HTTP/Chromium проверки не успешны, `chapter_ads_purchase_enabled` и `chapter_ads_billable_enabled` **false**. Legacy advertising тоже требует fully prepaid и стабильного posting. Никаких реальных денег по умолчанию. Фиксированная продуктовая доля, коммерческий прайс/его публикация и правомерный внешний источник кошелька остаются подписываемой конфигурацией владельца, а не выдуманными значениями из этого документа.
