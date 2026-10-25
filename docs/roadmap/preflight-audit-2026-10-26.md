# TOR — аудит полноты роадмапов перед разработкой и бета-запуском (26.10.2026)

**Результат:** 117 задач и 69 S в CI доказывают классификацию, **не** полноту спецификации и не работоспособность. Проверены [61 функция NEwBOO](https://app.notion.com/p/6afdef6d407c4be6b7f184a49d0408f3), [69 сценариев](https://app.notion.com/p/ea10f617944046b6bf37dc8473f41c2d), [tasks.tsv](tasks.tsv), [api-v1.md](api-v1.md), [data-and-events.md](data-and-events.md), git tree NEwBOO `17125ef758` и профильные технические аудиты.

**Машинная карта 61 функции:** [functions-coverage-audit.tsv](functions-coverage-audit.tsv). На момент начала проверки: **25** функций имеют базовые задачи; **24** имеют задачу, но недостаточную детализацию; **4** сценовые функции заморожены; **3** исторические исключены; **2** заменяются новой архитектурой; **2** зависят от внешнего провайдера; **1** является исследованием из PR. Эти числа о глубине **плана**, не о готовом Rust-коде. Три новых подробных договора ниже снимают часть пробелов, но не означают реализации или утверждения денежных рисков.

## 1. Выявленные дыры, порядок устранения

| Приоритет | Функции/сценарии | Что было в TOR | Что отсутствовало | Дополненный исполняемый документ |
| --- | --- | --- | --- | --- |
| **P0** | **Реклама F031/F032**, S042/S043 | F-10/F-11, D-11, один `/ads/campaigns` | ДВЕ модели: реклама за дни и Chapter Ads по оплачиваемым показам, финансовое резервирование, частичные сбои, publisher payouts, бюджет и anti-fraud. | [ADS-01…16](ads-execution-contract.md), F+D+G+H+I |
| **P0** | **Доступ F025/F055**, S005/S031/S032/S034/S065 | A/C/D с общим `CanRead` | Глава pinned V1 ≠ Layer CURRENT; lifetime/30d term, refund, creator withdrawal, hard moderation, отзыв купленного, закрытые previews/cache/worker. | [ACL-01…06](access-authorization-contract.md), A+B+C+D+E+G+H+I |
| **P0** | **Возвраты/финансы F027/F028/F029/F052**, S034–S039 | D-02…D-11 | Buyer refund и обязательный system disposition, удержания, доступный доход и одновременное approval. | D typed refund+settlement state machine, обязательный PG-test paid+revoked |
| **P0** | **Версии/публикация F019/F054**, S021/S022/S058 | B-03…B-07 | Удаление/withdrawal при оплаченной V1, publisher vs moderation, расписание и V1→V2 outbox, гонка с покупкой. | B/G/D/ACL миграции и race tests |
| **P0** | **Внешнее пополнение F061**, S041 | D-10 disabled | Нет подтверждённого Boosty provider contract, signature/callback/reconciliation. | Limited beta без real topup; публичная финансовая полнота остаётся BLOCKED |
| **P1** | **Рекомендации F010/F033/F034/F047**, S008/S044/S045 | F-05/06/07 + G-10 | 4 источника кандидатов, нормализация, adaptive hybrid, cold start, privacy, materialized generations, rollout/rollback, query budgets. | [REC-01…14](recommendations-execution-contract.md), F+G+H+I |
| **P1** | **События/аналитика F035/F056**, S046 + внутренний funnel | F-12 | Pageview ≠ book_open ≠ meaningful_read ≠ complete; idempotency, право на событие, retention/anti-fraud. | F/G/I единый event catalog и policy |
| **P1** | **Импорт/медиа F020–F022/F045**, S023–S025 | G-01…09 | Реальный allowlist источников и fixture парсеров; выбор долговечного storage/S3, pin/GC/restart/retry. | G-02/03/05/09, source/asset acceptance corpus |
| **P1** | **Социальная часть F011/F014/F017/F053** | E/G | Комментарии/оценки подборки не равны книге, lifecycle Club Factory, Inbox≠DM≠support, privacy/digest fanout. | E/G/H отдельная social permission+delivery матрица |
| **P1** | **Рефералы F039**, S052 | F-09+D-11 | Attribution window, trusted conversion, anti-fraud, hold/chargeback. | F/D posting+PG negative tests |
| **P1** | **Каталог/cache/SEO F004/F005/F049** | F-01–04/08 | L1/L2/SWR invalidation после block/withdraw, no hidden search hints, rollback generation. | F/G staged projections + real query plans |
| **P1** | **Эксплуатация F044–F048/F050/F051** | I + G + S0 CI | Full TOR PG/schema/Chromium/backup/checkpoints ещё нет; protection main — отдельный GitHub Ruleset, не просто зелёный workflow. | I-03/I-05/I-07/I-10/I-12 |

**Область не расширяем случайно:** интерактивные сцены F040/F041/F042/F060 заморожены, Shadow F059 исключён, диагностический Boosty F030 не равен F061. F057 — PR-only исследование, F058 — draft документация; YDB F044 заменяется PostgreSQL, Python deploy F050 — Rust-релизом.

## 2. Конкретные оставшиеся подготовительные решения

**Уже добавлено:** индекс 61F→S→задачи, два независимых ad-пайплайна и 16 исполнительских пакетов, 14 исполнительских пакетов рекомендаций, матрица доступа с 6 пакетами и запретами paid revoke/delete. Это уточнения **существующих** F-10/F-11/F-07/D-05/C-01, а не ещё одна независимая доска задач.

| ID | Владелец | Какой выход нужен ДО того, как исполнитель тронет рисковую ветку |
| --- | --- | --- |
| **PREF-01** | D/F/I | Подписанная единая policy точного учёта `RUB micros → копейки`, остатков не делящегося `price/target`, escrow/refund и publisher revenue. До этого `chapter_ads_billable/settlement_enabled=false`. |
| **PREF-02** | D/I | Фиксированные типы refund eligibility, system disposition, exchange/holds, финансовое созревание/payout; DB schema/контракт и негативная PG-матрица. Запрещён raw revoke paid grant. |
| **PREF-03** | A/B/C/D/E/I | Typed `CanDiscover/CanPreview/CanRead/CanPurchase/CanEdit/CanRefund`, миграции `is_withdrawn`/`Layer current version`/`term provenance`, единая [матрица](access-authorization-contract.md). |
| **PREF-04** | F/G/I | Golden dataset NEwBOO для scoring/rollout, PG generations/edges, chosen scenario weights from original code. Не делать новый popularity-only алгоритм. |
| **PREF-05** | G/I | Перечень фактически поддерживаемых источников URL, fixture каждого парсера, production durable object storage/credentials/backup/restore; интерфейс файлов без прямой отдачи secret/paid asset. |
| **PREF-06** | F/G/E/I | Раздельные события `page_view/book_open/read_start/meaningful_read/complete/ad_viewable`, trusted source и retention/privacy. Для meaningful_read требуется доказуемое правило, а не любой GET. |
| **PREF-07** | I/H | Из письменного `api-v1.md` сделать typed contract и contract tests/машинный OpenAPI, одинаковые поля JSON/HTML/PG. Общий ACL/финансы не пропускаются по changed-path. |
| **PREF-08** | I/владелец продукта | Зафиксировать профиль запуска: **ограниченная бета** с отключённым внешним платежом/billable ads и явно частичными рекомендациями либо **полный функциональный паритет** с обеими рекламами и реальной финансовой инфраструктурой. Без решения «15 дней» ≠ обещание 100%. |

Решение PREF-01/02/05/06/08 затрагивает настоящие деньги, конфиденциальность или внешнюю инфраструктуру; его нельзя подменить удобной технической догадкой. Независимые технические ветки можно начать с безопасными feature flags и fixtures, но их нельзя объявить соответствием публичному продукту.

## 3. Контрольные точки 15-дневного плана

| Срок | Обязательные факты, не обещания |
| --- | --- |
| **Д0, до предметных веток** | Таблица всех 61 F, выбранный `limited/full` профиль, ADS/REC/ACL contracts; общий owner и API/schema, стабильный исходный SHA. |
| **Д1–Д2** | TOR настоящий PostgreSQL+регистрация+reader, S0 RAM-demo не считается. |
| **Д3–Д5** | D деньги/Layer, B publish/withdraw, G outbox, H браузер; ad/rec не считаются выполненными по форме. |
| **Д6–Д9** | PG/Chromium: 30d/lifetime, прямой paid V1, double purchase, cold-start, hidden, 60%-1s impression. |
| **Д10–Д12** | Concurrency, hard ad budget caps, settlement+refund reconciliation, recommendation generation rollback, восстановление DB/worker, full release suite на одном SHA. |
| **Д13–Д15** | Бета только по реально прошедшим feature flags; отдельная матрица 61F/69S. Полный публичный выпуск без красных P0. |

## 4. Жёсткие блокеры беты

- Скрытый/paid reader body доступен через GET/preview/history/cache/media/ads/recs без grant — **BLOCK**.
- Списаны деньги без ledger и права, отозван оплаченный доступ без system disposition, удалена купленная версия — **BLOCK**.
- Реклама billed без подтверждённого 60%/1s, превышает target/budget, settlement даёт двойное начисление или теряет unused escrow — **BLOCK**.
- Рекомендации выдают заблокированную/private/18+ книгу, показывают приватные социальные сигналы — **BLOCK**.
- Crash/restart удваивает операции/теряет очередь; live PG/Chromium не прогонялись; stage rollback не проверен — **BLOCK**.
- S041 Boosty отсутствует, но публично объявлен работающим пополнением — **BLOCK**.

**Вывод:** список областей в TOR почти полный, **глубина плана была недостаточной** для денег, рекламы, рекомендаций и paid Layers. Новые контракты закрывают крупную часть технической неопределённости. Оставшиеся перечисленные продуктовые решения и фактические CI/PG/browser доказательства нужно считать отдельными допусками, а не поручениями «как-нибудь проработать» при реализации.
