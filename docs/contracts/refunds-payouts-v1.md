# TOR FIN v1 — возвраты, обмены, доли, удержания, выплаты и восстановление (FIN-02)

**Нормативная подготовка, не реализованный money engine.** Владелец **D**, shared DDL/transaction gate — I, книга и редакция — B, право чтения — C, проверка модерации — E, outbox/recovery — G, браузер — H. Сохраняет NEwBOO [N13-01…11,N14-04…09](https://app.notion.com/p/3cd32a84beec810b945bd914417a180f), [MONEY-03](https://app.notion.com/p/40432a84beec814c87c5e35dd8ed1dcd), [paid chapter withdrawal](https://app.notion.com/p/3cd32a84beec810eaae3e4460bd71df3), [Layer lifecycle](https://app.notion.com/p/3cd32a84beec81e4af2af311a284f780). Главные старые задачи: D-02…D-09,D-11,D-13; S031–S039,S041.

## 1. Термины и единственные источники истины

- `purchase` — состоявшийся immutable факт; `purchase_lines` при пакетной покупке, одна строка на точный тип entitlement. Покупка фиксирует `actor_id,object_id,version_id/offer_revision,amount_minor,RUB,share_snapshot,refund_policy_version,refund_deadline_at`. Текущая цена или состав команды **не** меняют старые факты.
- `ledger` — append-only движения: debit/credit/reversal; `(idempotency_scope,key,payload_digest)` и `(source_type,source_id,effect_type)` уникальны. Баланс — проверенная материализация ledger, не альтернативная mutable truth.
- `entitlement` — текущий допуск, выведенный из действительного purchase/source policy; `reader_progress` никак не создаёт права.
- `holds` — временные финансовые ограничения (immature creator earning, pending payout, disputed refund/chargeback, ad escrow). Пополнения кошелька не исключаются из расчёта выплаты только по причине их происхождения. `withdrawable` получается вычитанием актуальных holds и reservations из общего расчётного баланса.
- `refund` — урегулирование **конкретной покупки**, не «изменить любой balance». `system_disposition` — доверенное принудительное урегулирование T&S/legal, не bypass обычного user-refund.
- `receipt evidence`: проверенная исходная запись posting со совпадающими owner/amount/currency/direction/source/id; факт наличия случайного `transaction_id` или `COUNT>0` — НЕ финансовое доказательство.

## 2. Конечные автоматы (точные переходы)

| Сущность | Переходы | Запрет |
| --- | --- | --- |
| RefundRequest | `pending→processing→approved/rejected`, допускается `processing→pending` при recoverable lease | approved/rejected terminal; второй open `pending/processing` на одну purchase = 409 |
| Purchase | `paid→refunded` либо `paid→exchanged` через canonical reversal/transfer | повторная смена terminal; `paid→revoked` без money disposition |
| Exchange | `requested→validated→posted` или rejected; источник refund-like | другой actor/новая цель без заново проверенного offer |
| SystemDisposition | `queued→processing→settled` или `retry/quarantined` | completed без financial evidence; ordinary refund deadline НЕ ограничивает T&S |
| PayoutRequest | `pending→approved_for_processing→internal_posted→external_confirmed` либо `pending→rejected` / quarantine | пользователь никогда не создаёт `internal_posted`; внутренний posted ≠ подтверждённый банковский перевод |
| RevenueShareSnapshot | immutable at purchase; `earning→immature→mature→reserved→payout_posted`, возможен reversal/debt | нельзя изменить уже начисленную долю после team ownership transfer |

**Отдельные статусы для внешней выплаты:** пока нет доказанной интеграции банка/провайдера, публичный `cash_out_enabled=false` и нельзя утверждать `external_confirmed`. Внутренний операторский `internal_posted` — финансовый факт в БД, не внешняя квитанция. Вывести с баланса **до** реального settlement можно только если договор содержит escrow/подтверждённую компенсацию; в первой бете без этого pipeline оператор оставляет заявку в `pending`.

## 3. Право пользователя на обычный возврат: одна критическая транзакция

1. `POST .../refund-request` проверяет Actor владелец purchase, если status=paid, отсутствие open refund; reason непустой, длина ограничена, `refund_policy_version` и `refund_deadline_at` записаны при покупке. Если Policy отсутствует — не угадывать срок, `422 REFUND_POLICY_UNAVAILABLE` без сторонних эффектов. После terminal rejected новую заявку создать можно по текущей ещё действительной policy.
2. **First paid exposure barrier**: перед выдачей защищённого контента C атомарно/CAS выставляет `opened_at` в D purchase/library факт при первом действительно успешном reader access. Для direct S3 grant этот момент — первая выдача успешно авторизованного ключа+URL (консервативный барьер; уже полученный контент нельзя вернуть). Если grant не выдавался из-за ошибки S3/PG — не выставлять `opened_at`. Разница между issuer и S3 GET не позволяет достоверно доказать просмотр, но предотвращает refund-after-access.
3. При `admin approve` D **в той же serializable транзакции** перечитывает `paid`, ownership, refund policy+deadline, `opened_at` и активный lock; если начат доступ `request→grant→approve`, результат `409 REFUND_ALREADY_OPENED` (заявка не одобрена и деньги/право не изменены).
4. Успех: reversal оригинальных buyer/creator/platform entries по immutable split; `purchase=refunded`, `entitlement=revoked`, `refund=approved`, `refund decision(actor,reason,revision)` и `RefundCommitted` outbox в **одном PG commit**. Фоновые уведомления/метрики после commit не определяют финансовый успех.
5. Refunded owner получает refund ровно на `amount_minor_at_purchase` (не текущую цену), меняет доступ V1 / защищённые grants. У уже выданного S3 bearer URL возможен до 90 сек срок жизни; *предъявленный ключ или сохранённый plaintext пользователь способен копировать* — не обещать DRM.
6. `reject` — trusted transition без reversal и без изменения purchase; 2 одновременных moderator calls → один terminal outcome. На другого пользователя request → 404, не раскрывать покупку.

## 4. Системная блокировка, оплаченные версии, Layer

- **Creator withdrawal** (`is_withdrawn=true`): больше не продаётся/не виден public; старый direct paid V1 доступен. Hard delete купленной V1 → `409 PAID_ENTITLEMENTS_EXIST`. Paid S3 object и wrapped DEK pinned/не GC, пока существует обязательство.
- **T&S/legal permanent block**: deny grant немедленно; G получает durable очередь затронутых Purchase IDs; каждый `system_disposition(id=purchase_id,reason=content_permanently_removed)` проходит уникальную trustworthy reversal **без** `refund_deadline`/`opened_at`. Недоказанный внешний источник денег/невозможный reversal → quarantine + operator alert, не помечать `settled`. Только после всех obligations можно GC.
- **Layer**: продажа фиксирует `layer.current_version_id + offer_revision + price + pricing_type`; `lifetime` и дискретные `prepaid_30d` terms. 30d renewal — **новая purchase+term**, никакого продления того же shared access с исчезновением старой покупки. Возврат term P1 не лишает права оставшийся P2. Активация новых книг active entitlement — version-driven, не O(all subscribers) authoritative fanout.
- Админ не может `raw revoke` access с `provenance_type=purchase`, включая **нулевую цену**, даже если нет ledger entry. Для `grant` возможен audited direct revoke. Гонка `grant→purchase` vs admin revoke исключается lock/CAS в той же authority.
- Exchange не создаёт новое право/лишнее начисление, пока оригинальный reversal не подтверждён; новая версия/книга/ценовая разница — отдельный immutable offer snapshot с permission check.

## 5. Доходы и выплаты: единая модель NEwBOO (продуктовое решение 2026-11-09)

Для расчёта доступной к выводу суммы используются общий кошелёк и авторские начисления. **Запрет на вывод пополнений по их происхождению не вводится.** Для точной сверки с NEwBOO используется формула `balance - holds - pending_payouts`.

```text
accrued_creator_net = SUM(creator_share_earnings) - SUM(confirmed_creator_reversals)
unified_balance = wallet.balance_minor + accrued_creator_net
current_holds = immature_creator_net + open_refund_and_chargeback_holds + other_financial_holds
withdrawable_minor = max(0, unified_balance - current_holds - active_pending_payout_reservations)
```

При этом учёт immutable source/transaction/claim сохраняется для расследования возвратов и аудита, **но не используется как запрет на выплату обычных пополнений**. Суммы не округляются через floating point, а повторное создание заявки сериализуется на уровне PostgreSQL.

**Этап интеграции:** текущий TOR отдельно хранит `reader_wallet_accounts` и `finance_creator_earnings`. В S038 PR #753 их объединяет расчётная проекция для заявки и финансового кабинета. Физическая проводка авторских начислений в расходуемый кошелёк (покупки из этих денег, безопасные reversal и запрет одновременного расходования ожидающего резерва) требует отдельной транзакционной миграции D-08/D-06 и приёмки. Нельзя заявлять полную идентичность движения денег с NEwBOO, пока она не выполнена.

Карточка «Финансы» сохраняет историю уже начисленных средств после выхода из команды; создание **новой** заявки может требовать текущего статуса участника по правилу NEwBOO. Внешняя выплата до подписания условий остаётся выключенной.

### D-08/S038: зачисление авторского дохода в расходуемый кошелёк

- Стандартная покупка использует `reader_wallet_accounts` и неизменяемый `reader_wallet_entries`. Как и в NEwBOO, созревшее авторское начисление может стать внутренним балансом для покупки, а не только расчётной суммой вывода.
- Новая отдельная таблица `finance_creator_wallet_credits(purchase_id,user_id,amount_minor,wallet_entry_id)` гарантирует **одну** денежную проводку на неизменяемую долю продажи. Она ссылается на финансовый origin D-08 и точную запись кошелька. PostgreSQL deferred-триггеры отклоняют неподтверждённую проводку без исходного начисления и одиночную запись в журнале без проводки.
- До зачисления обязательны `clear_at`, завершённый refund window, отсутствие открытого возврата и подтверждённого reversal. Два вызывающих сервиса не создают две проводки; транзакционная блокировка кошелька и первичный ключ обеспечивают idempotent replay.
- Финансовая сводка вычитает **уже зачисленные** суммы из ещё не зачисленных созревших начислений. Без этого `wallet_balance + creator_earnings` посчитал бы одну сумму дважды.
- SQL-триггер запрещает любое уменьшение `reader_wallet_accounts.balance_minor`, если после него общий доступный остаток с учётом уже зачисленных и ещё созревших начислений станет меньше действующих payout reservations или clawback debt. И single checkout, и batch checkout используют эту защиту; соперничающий payout request синхронизируется через блокировку строки `users`.
- **Граница:** сервис физической проводки — внутренний и не вызывается публичным HTTP. Автоматическое зачисление, внешняя выплата, комиссионная политика и UI для перевода остаются вне этого среза до приёмки. Никаких changes production/VPS или запуска SQL migrations с реальной БД.


`POST /api/v1/me/payout-requests`:
- stable Idempotency-Key из 43 символов; второй такой же payload получает ту же pending заявку/резерв. Подменили amount — 409;
- сумма положительна и ≤withdrawable; `pending` резервируется атомарно, ***без немедленного debit***;
- `approve` только trusted admin/system со step-up и reason; в одной PG transaction проверить effective capacity с исключением **собственного** резерва, exact money/owner/currency, записать debit+posted/internal receipt, перевод reservation в consumed. Reject резерв освобождает;
- внешняя оплата/интеграция + receipt — **вне доказанного TOR v1**; до provider verification флаг публичного cash-out OFF, никаких кнопок «деньги выплачены» по одному внутреннему статусу.

## 6. PostgreSQL: схемы и индексы (владелец миграций I)

```sql
CREATE TABLE finance_refund_requests (
 id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
 purchase_id BIGINT NOT NULL,
 user_id BIGINT NOT NULL,
 state TEXT NOT NULL CHECK (state IN ('pending','processing','approved','rejected')),
 reason TEXT NOT NULL,
 expected_revision BIGINT NOT NULL DEFAULT 0,
 idempotency_key TEXT NOT NULL,
 created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
 decided_at TIMESTAMPTZ,
 decision_actor_id BIGINT,
 decision_ledger_transaction_id BIGINT,
 UNIQUE(user_id,idempotency_key)
);
CREATE UNIQUE INDEX finance_one_open_refund_per_purchase
 ON finance_refund_requests(purchase_id)
 WHERE state IN ('pending','processing');
CREATE TABLE finance_payout_requests (
 id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
 user_id BIGINT NOT NULL,
 amount_minor BIGINT NOT NULL CHECK (amount_minor>0),
 currency TEXT NOT NULL DEFAULT 'RUB' CHECK(currency='RUB'),
 source_origin TEXT NOT NULL,
 state TEXT NOT NULL CHECK(state IN
   ('pending','approved_for_processing','internal_posted','external_confirmed','rejected','quarantined')),
 intent_key TEXT NOT NULL,
 intent_digest BYTEA NOT NULL,
 reservation_minor BIGINT NOT NULL,
 internal_ledger_transaction_id BIGINT,
 external_provider_receipt TEXT,
 created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
 UNIQUE(user_id,intent_key)
);
CREATE TABLE finance_earning_holds (
 hold_id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
 user_id BIGINT NOT NULL,
 source_type TEXT NOT NULL, source_id TEXT NOT NULL,
 origin_type TEXT NOT NULL,
 amount_minor BIGINT NOT NULL CHECK(amount_minor>=0),
 clear_at TIMESTAMPTZ NOT NULL,
 released_at TIMESTAMPTZ,
 UNIQUE(source_type,source_id,user_id)
);
```

Это **предлагаемые дополнительные таблицы** для I, не duplicate wallet ledger и не команда создавать schema вне D миграций. `finance_payout_requests` имеет immutable amount и explicit state; `approval audit` и `system_dispositions` отдельные таблицы D с уникальным `(purchase_id,reason,event_kind)`. Backfill legacy paid grants неизвестной provenance **fail-closed**, а не массовая автокомпенсация.

## 7. Точные испытания FIN-B01…B12

| ID | Последовательность | Обязательный исход |
| --- | --- | --- |
| B01 | same user/key payout дважды; второй amount иной | одна reservation; второй 409 |
| B02 | pending payout, разлогиниться/потерять team role | запись и sticky Finance остаются, creator ACL не появляется |
| B03 | wallet=100, mature_creator=60, hold=20, pending=10 | unified balance=160, withdrawable=130; approve self reservation не вычитает 10 дважды |
| B04 | два администратора одновременно approve одного payout | один debit, terminal posting, одна квитанция |
| B05 | request refund→успешный S3 paid grant→approve | fail REFUND_ALREADY_OPENED, ledger/purchase unchanged |
| B06 | два refund request одной purchase | open row один по unique partial index |
| B07 | два moderator approve/refund и retry after process crash | один reversal, entitlement revoked once |
| B08 | creator удаляет paid V1 / withdrawal | DELETE=409, withdrawal preserves pinned ciphertext V1 |
| B09 | T&S blocked purchase после первого просмотра | system disposition bypasses обычное refund window, audited reversal |
| B10 | Layer 30d P1+P2, вернуть P1 | P2 сохраняет paid rights; future additions по сроку |
| B11 | corrupted receipt / чужой recipient / legacy shared grant | quarantine, нет ложного posted/revoked |
| B12 | ad settlement pending/creator pending hold, provider unavailable | cash-out button disabled; никакого фиктивного external_confirmed |

**Приёмка:** FIN-B на реальной PostgreSQL с параллельными соединениями + HTTP/Chromium deny/grant, crash injection и SQL audit. Не объявлять полную коммерцию NEwBOO, пока отсутствует внешнее подтверждение пополнения и выплат. Содержание договора исполнителю **не** передаёт право самостоятельно выбрать условия или срок возврата: deadline берётся из сохранённого `refund_policy_version` и immutable offer; если policy/catalog ещё не опубликованы, операция возвращает документированную ошибку и остаётся выключенной.

## 10.11.2026 — S036: возвраты только администратором через поддержку

Решение владельца заменяет пользовательскую вкладку заявок. GET/POST `/me/refunds`
удалены из маршрутизатора. Читатель использует `/support`, а администратор
`/admin/finance/refunds` для поиска по имени/handle/ID, изучения покупок и
выбора точного `chapter_purchase_facts.id`. Каждая позиция пакетной покупки
имеет самостоятельный purchase ID и историческую стоимость.

Только у подтверждённого диалога поддержки с реальным сообщением самого
пользователя администратор может создать одно неизменяемое намерение возврата,
указав основание. Это не движение средств. После отдельного подтверждения
текущим паролем и one-use session proof D-06 в одной PG-транзакции выполняет
reversal оригинальной суммы, запись кошелька, creator/platform reversal,
revoke доступа и журнал. Повтор не зачисляет второй раз.

Решение поддержки `support_discretion` — отдельное доверенное основание:
не переписывает/не изобретает старую потребительскую refund policy. Оно может
быть применено администратором к уже открытой главе, но не может забрать
у пользователя уже скопированный текст. Обычные consumer policy проверки
first-exposure и deadline остаются обязательными вне административного основания.
Не менять прошлые purchase facts; не заявлять внешнее возвращение денег.
Здесь реализованы только возвраты отдельных купленных глав (в том числе
отдельных строк batch). Layer/рекламные/другие типы требуют собственных
доказанных финансовых reversal, не маскировать их под chapter purchase.

Финансовые POST выключены по умолчанию `TOR_ADMIN_SUPPORT_REFUNDS_ENABLED`,
плюс `TOR_PAID_CHECKOUT_ENABLED`; запуск требует отдельного допуска.
Без деплоя и постоянных миграций.
