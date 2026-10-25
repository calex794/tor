# TOR — единая система доступа: исполняемый контракт и матрица отказов

**Владелец A + B + C + D + E, сборка I.** Привязка: S005, S031, S032, S034, S036, S037, S039, S049, S065; исторические F024/F025/F027/F028/F043/F055. Это **спецификация к реализации**, не существующий Rust-компонент. Исходные аудиты: [слои и продления](https://app.notion.com/p/3cd32a84beec81e4af2af311a284f780), [слой: visibility/admin revoke](https://app.notion.com/p/3cd32a84beec81fcac28c0d7e2cb0735), [withdrawn/pinned chapter](https://app.notion.com/p/3cd32a84beec810eaae3e4460bd71df3). Код: [chapter_access_service.py](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/services/chapter_access_service.py).

## 1. Никакого универсального `has_access`

Интегратор I определяет в `tor-contracts` независимые операции:
- `CanDiscover(book/episode,Viewer)` — разрешено ли увидеть публичные метаданные;
- `CanPreview(ChapterVersion,Actor)` — автор/редактор видит preview в защищённом **авторском**, а не reader endpoint;
- `CanRead(ChapterId,ReaderContext) -> {version_id,grant_kind} | Denied` — фактическое право на байты;
- `CanPurchase(OfferSnapshot,Actor)` — право начать конкретную покупку по текущему предложению;
- `CanEdit(BookId/ChapterId,Actor)`, `CanModerate(Target,Actor)`, `CanRefund(PurchaseId,Actor,Reason)`;
- `CanReadFile/Preview/CacheKey` — те же права применяются к media, API, фоновой выдаче, HTML и cache.

**Базовая иерархия проверок на reader:** (1) достоверный Actor/guest; (2) существование материала, отдельная проверка `hard_deleted/legal/admin_hold/blocked` → hard deny на нормальном reader; (3) разрешённый author preview **только в отдельном авторском маршруте**; (4) явный purchased chapter pin V1 + не revoked; (5) active paid Layer на книги; (6) public/free current edition; (7) deny. Не использовать шаг (4)/(5) для обхода (2). Guest не должен узнать, какой скрытый объект существует.

**Отдельная покупка:** алгоритм не вызывает `CanRead` как разрешение списать деньги: `CanPurchase` проверяет offer/status/actor/version/price **в той же PG-транзакции, где пишется wallet/ledger/purchase**, без TOCTOU. Для выдачи платных байтов понадобится отдельная проверка гранта; факт появления позиции чтения не является грантом.

## 2. Таблица решений (требуемые тесты)

| Контекст | Каталог | Прямая глава/файл | Новая продажа | Специальный эффект |
| --- | --- | --- | --- | --- |
| Гость, public/free | видно | текущая published свободная V | не требуется | guest progress не долговечный Actor |
| Гость, public/paid | метаданные/цену видно | нет текста, 401/404 по правилам приватности | вход обязателен | ни snippet, ни CDN не содержат байт paid |
| Владелец, черновик | только авторский кабинет | preview в author-route | нельзя | автор не получает право на чужую цену |
| Чужой редактор, черновик | нет | 404 | нельзя | роль члена другой команды не помогает |
| Прямая покупка главы V1 | публичная мета по видимости | **V1**, даже если появилась V2 | не повторять автоматически | pinned version_id хранить неизменно |
| Покупка главы V1 после refund/chargeback | как для остальных | **нет**, пока ревокация действует | новая покупка — отдельная команда | история покупки append-only |
| Layer lifetime, active | публичный слой доступен | **CURRENT** разрешённых глав **в книгах, когда-либо добавленных в слой во время права** | новая покупка не нужна | будущие книги добавляются к lifetime |
| Layer prepaid-30d, active | публичный слой доступен | CURRENT в книгах, добавленных в оплаченный период | продление отдельным term | future additions только до конца оплаченного term |
| Layer 30d expired | мета зависит от публичности | нет оплаченного layer права | новый 30d term после проверки offer | не наследовать добавления после expiry |
| Layer удалён из витрины владельцем (hidden) | 404 для нового покупателя | **сохраняется существующее paid Layer право** в разрешённых книгах | запрет | кабинет без dead `/layers/{id}` CTA |
| Layer platform-blocked | no new storefront/sales | доступ по купленному Layer **не отзывается только фактом блокировки витрины**; ниже book/chapter hold побеждает | запрет | owner не может снять blocked |
| Глава creator-withdrawn, прямая оплаченная V1 | не показывать новым | действующий прямой paid pin V1 читается | новая продажа запрещена | creator DELETE с paid obligations → 409 |
| Глава creator-withdrawn, Layer читатель | не показывать новым | Layer даёт CURRENT chapter state, не V1 pin; withdrawn не создаёт вечный доступ к старой редакции | нет | direct purchase и Layer не отождествлять |
| Глава/platform legal hold, blocked или admin_hold | скрыто | отказ и для purchased в обычной читалке | запрет | отдельная trusted financial disposition для permanent removal |
| Задержанная публикация | публичный TOC не показывает текст до срока | до срока — отказ, исключение авторский preview | no new sale | worker проверяет актуальный срок и роль |
| Личная/private книга | нет | только владелец и явно разрешённые соавторы | нет | приватное содержимое не попадает в поиск/рекламу |

**Контроль всех поверхностей:** SSR reader, JSON `/read`, `/reader`, `/me/library`, direct version/history, preview, offsets/autosave, comment anchor/snippet, SEO/search, recommendation/ads card, S3/storage signed URL, offline feed, CDN/browser cache. Каждая получает одинаковое deny-поведение, независимо от количества обходных маршрутов.

## 3. Слои — конкретный оплаченный договор из аудита NEwBOO

- `layer_versions` неизменяемо фиксируют **состав книг**, не содержат version текста главы и не являются price snapshot; `layers.current_version_id` — authoritative pointer, **не** `MAX(random layer_version_id)`.
- У коммерческого предложения отдельный `offer_revision` с `current_version_id,price_minor,currency,pricing_type,sale_status`. UI показывает этот snapshot; POST подтверждает его в самой денежной транзакции. `hidden/blocked` нельзя купить даже через прямой API.
- `one_time` → бессрочное Layer право + будущие добавленные книги. `prepaid_30d` → отдельный оплаченный term продолжительностью 30 суток от начала указанного срока; future additions только пока term активен. Изменение `pricing_type` с непогашенными обязательствами в V1 запрещено.
- **Раздельные термы:** отдельная purchase/renewal запись на каждый 30d term; возврат одной покупки отменяет **только её** contribution, но не уничтожает другой действующий term. `lifetime` refund пересматривает право только если нет иных поддерживающих paid purchases. Обратная ссылка `access←purchases` с индексом обязательна.
- Owner `hidden` закрывает новые продажи, но не отзывает уже оплаченные книги. Owner не может сделать `blocked` или снять блокировку платформы. Административный raw revoke `purchase-backed` запрещён даже при `price=0`; только `grant` можно отозвать напрямую.
- Для `layer_accesses` хранить `provenance_type=grant|purchase` (не путать со сроком `entitlement_mode=lifetime|prepaid_30d`). Покупка переводит `grant→purchase` в той же критической транзакции; unknown/null provenance → отказ. Назначать purchase provenance по фактам при backfill; **не** делать массовые авторефанды.
- Избежать `O(all subscribers)` на каждом изменении состава: версия имеет монотонный `version_seq` и `published_at`; grant Layer проверяет индексированный факт «book содержалась в одной из версий слоя, опубликованных во время действующего lifetime/paid term» без массового fanout. Для обратной совместимости materialization может быть производной проекцией, но **не** authority прав; прежние купленные книги не теряются из-за пропущенного задания G.
- **Единицы:** chapter direct purchase pin V1, Layer entitlement на книги CURRENT — не синхронизировать автоматически и не выдавать одно за другое.

## 4. Оплаченная глава и отзыв/удаление

- Новая редакция V2 не меняет прямой paid purchase V1. Переключение active library version — отдельное пользовательское действие, не автоupgrade.
- Создание `chapters.is_withdrawn BOOLEAN NOT NULL DEFAULT false` отдельно от статуса/`admin_hold`. Creator withdrawal: no future sales/catalog/free delivery, но прямой оплаченный V1 остаётся читаемым при отсутствии более сильной блокировки.
- Destructive DELETE с активным оплачиваемым правообладателем → `409 PAID_ENTITLEMENTS_EXIST` и **ни одна** запись главы/версии не мутирует; вариант owner withdrawal — явная операция.
- Наличие paid references → версия и файл GC pinned; нельзя удалить объект и оставить purchase-backed право с 404 файлом.
- Permanent T&S/legal removal: **сначала hard deny**, затем durable операция по affected purchase IDs → стабильный disposition key → финансовый reversal или отдельно выбранная легальная компенсация → аудит → после завершения обязательств storage cleanup. Возврат системой не зависит от обычного срока/правила `refund before first open`.
- На PG транзакциях запретить гонку `purchase vs DELETE/withdraw`: writer на общей chapter row с `FOR UPDATE`/CAS, purchase перечитывает hold/withdraw/current price после lock, деньги и право commit вместе. Не ставить lock только на первый GET UI.

## 5. Исполняемые пакеты и сдача

| Пакет | Владение | Итоговые файлы | Приёмка |
| --- | --- | --- | --- |
| ACL-01 | I + A | `crates/contracts/src/access.rs`, `crates/accounts/src/services/actor.rs` | одна typed decision и matrix-driven unit tests |
| ACL-02 | C | `crates/reading/src/services/access.rs` + HTTP guards | все 16 матричных случаев, прямые URL/Cache fail-closed |
| ACL-03 | B | `crates/books/src/services/withdraw.rs`, миграция через I | owned paid V1 => withdrawn still readable; DELETE=>409; race test |
| ACL-04 | D | `crates/commerce/src/services/layers.rs` + indexed PG layer versions/terms | lifetime/new book, 30d expiry, wrong current pointer, refund renewal isolated |
| ACL-05 | D + E | `crates/commerce/src/services/disposition.rs` + E moderation reason | admin raw revoke on paid/zero-price rejected; audited system disposition |
| ACL-06 | G/H/I | `web`, CDN/media, F index/ads checks | 2 actors, cached URL/preview/SEO/ads cannot leak pinned paid chapter or withdrawn |
 
**Связанные базовые задачи:** A-04/A-05, B-04/B-05/B-11, C-01/C-07/C-10/C-11, D-02/D-03/D-05/D-06, E-10, G-09, H-05, I-02/I-08.

**Допуск к платной бете:** критические сценарии ACL-01…06 выполнены на **одном SHA** с реальной PostgreSQL, отрицательными запросами и браузером. Никакого «покупка есть, а бесплатный читатель открывает платное» или `paid+revoked без disposition`.


## 6. S3-совместимый контракт гранта и хранения — обязательное уточнение 26.10

**Документы:** [paid delivery v1](../contracts/paid-delivery-v1.md), [финансовые статусы/refund](../contracts/refunds-payouts-v1.md), [RIL/progress](../contracts/ril-reader-signals-v1.md). Этот раздел нормативно дополняет таблицу `CanRead`; прежние упоминания HTTP «тело главы» применимы к безопасному показу после WebCrypto в браузере, **не** к выдаче backend plaintext.

**API:** `GET /api/v1/chapters/{id}/read` возвращает только метаданные/`grant_required` для платных глав; `POST /api/v1/reader/chapters/{id}/delivery-grants` требует актуального Actor+Origin+CSRF, выбирает редакцию на сервере, перепроверяет D entitlement + B status в PG и возвращает короткоживущий S3 signed GET **ciphertext** и раскрывает только per-version DEK, никогда KEK. `Cache-Control: private,no-store`, signed URL/DEK не входят в HTML, SQL debug, RIL labels или аналитические payload.

**Семантика ошибок:** anonymous paid → 401 без grant; logged-in user without paid entitlement → 403/paywall без текста; private/held/deleted/unknown → 404, если существование нельзя раскрывать; refund/revoke/expired Layer → 403; stale expected revision/offer при покупке → 409; PG/S3/KEK unavailable → 503 и **никаких** signed URL/key. Из уже выданной ссылки пользователь может скачивать до TTL, максимум 90 sec default; это установленный остаточный риск bearer tokens, не обещание невозможного мгновенного отзыва байтов.

**Решение по cache:** (actor/session, chapter_id, resolved version_id, entitlement_revision, B visibility_revision) влияет на private grant outcome; *не кэшировать* решение после refund/revoke/hold и никогда не переносить result на другого Actor. Общий CDN/SEO разрешён только для публичных бесплатных bytes с немедленным invalidation при смене режима; private S3 ciphertext не публиковать даже при owner preview. Запрет делать прямой `storage_key` пользовательским ID или давать `presign_get` до D/B check.

**Порядок выдачи платного гранта:**
1. A валидирует Actor+CSRF; C запрашивает D typed `EntitlementDecision`; D проверяет committed purchase/не refunded/Layer term/provenance и выбранную **pinned V1** (direct) либо CURRENT (Layer).
2. B подтверждает отсутствие hard legal/moderation hold, состояние immutable version, статус архива/withdrawal, версии asset refs; G подтверждает private encrypted object+wrapped DEK, возможность создать signed URL в установленном TTL.
3. D атомарно/CAS фиксирует `first_paid_exposure_at` только при реально подготовленном выдаваемом grant (не при GET metadata и не при S3 preflight failure). Это барьер обычного возврата; drop HTTP после committed grant возможен и должен быть виден оператору по request_id для ручного рассмотрения, не обходиться скрытым авторефандом.
4. C возвращает `tor.paid_delivery.v1`; H получает ciphertext напрямую S3, WebCrypto проверяет GCM/tag/AAD, применяет gzip и protected renderer без localStorage/plaintext persistence; RIL классифицирует POST grant и PUT progress, **не** S3 GET.
5. PUT progress через C/D/B `ReaderVersionManifest` валидирует block_id точной V1/current версии. Только финальный валидированный блок создаёт deduped `ChapterCompleted`; no completed signal из grant.

**Денежные гонки:** при покупке D row-lock chapter offer+version и revenue snapshot до posting; при отзыве D row-lock purchase/entitlement и фиксирует reversal; при creator delete B использует совместимую транзакционную блокировку/сериализацию на той же главе. Нельзя держать transaction lock во время сетевого S3 GET; короткая транзакция на выдачу grant/барьер, HTTPS ответ после commit, известное окно bearer TTL.

**Приёмочные тесты ACL-S3:** U1 купил V1, U2 нет → only U1 получает DEK; U1 V1 остаётся после V2; U1 refunded/Layer expired → новые grants denied; held/blocked deny до grant при paid; direct private asset без подписи 403; bad AES tag/nonce/AAD без plaintext fallback; metadata/SEO/comments/notification не раскрывают paid text; S3 outage даёт 503 без first-exposure barrier; cancel и simultaneous refund/grant дают детерминированный один outcome. Всё на PG + совместимом приватном S3 + Chromium, не только тесты Rust.
