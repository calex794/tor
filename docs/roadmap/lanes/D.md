# D — Коммерция и права покупки: готовая инструкция TOR

**Статус:** пооперационный *план реализации*, не отчёт о готовом коде. Связанные карточки [Tela](https://telawiki.com/spaces/430/pages/7729/d-main) и каноническая [матрица сценариев](https://app.notion.com/p/ea10f617944046b6bf37dc8473f41c2d). Весь код только в TOR. Источники: [NEwBOO 17125ef758](https://github.com/HeisLuka/newboo/tree/17125ef758a791e9630505c14d24f7fe65aed228), [bokarust d11b27b431](https://github.com/calex794/bokarust/tree/d11b27b43125b93e59809147e8e687203fa0753b).

**Решения уже приняты:** [архитектура](../decisions-v1.md), [API](../api-v1.md), [схема PG, события и лимиты](../data-and-events.md), [режим тестов/фикстуры](../tests-and-handoffs.md). Фраза «сверить» в старой карточке означает **доказать соответствие выбранному контракту**, а не придумать новую архитектуру.

**Смежные линии:** B поставляет цену и immutable version, A Actor, C читает выданный grant, F передаёт ad/referral события, I даёт одну PG транзакцию и миграции.

**Пакет сдаётся** одной рабочей пользовательской вертикалью, финальным набором тестов и реальной интеграцией. На каждом промежуточном шаге исполнять только дешёвые format/check/contract проверки.

### D-01 — Д0–Д1

**Сделать:** Зафиксировать Money/WalletId/PurchaseId/EntitlementId, валюту, idempotency и версию цены; границу одной транзакции.

**Главный выходной файл:** `crates/commerce/src/domain/money.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** contracts I; commerce/domain+schemas; wallet/ledger

**Сценарий и старый код:** [S031](https://app.notion.com/p/40432a84beec81999c5debde42de045f) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/layers.py)), [S034](https://app.notion.com/p/40432a84beec8186b99dfdc62851fc6c) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py)), [S035](https://app.notion.com/p/40432a84beec8113b806d4ad851c932b) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_interactions.py))

**Принятый API/действие:** POST /api/v1/layers/{id}/purchase; POST /api/v1/purchases; POST /api/v1/purchases/batch

**Проверка:** Проверка запрета отрицательной суммы и двойного источника истины. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-commerce --test slices_pg d_01_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### D-02 — Д1–Д2

**Сделать:** Атомарное списание кошелька, append-only журнал и pinned право на ChapterVersion.

**Главный выходной файл:** `crates/commerce/src/services/checkout.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** commerce/services/checkout; repositories/wallet,ledger,purchases,entitlements

**Сценарий и старый код:** [S034](https://app.notion.com/p/40432a84beec8186b99dfdc62851fc6c) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py))

**Принятый API/действие:** POST /api/v1/purchases

**Проверка:** Два одинаковых ключа дают одну проводку и один entitlement. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-commerce --test slices_pg d_02_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### D-03 — Д2 P0

**Сделать:** Отказ доступа без покупки; сервис разрешения точной приобретённой версии для C.

**Главный выходной файл:** `crates/commerce/src/services/entitlements.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** services/entitlements; порт contracts через I

**Сценарий и старый код:** [S034](https://app.notion.com/p/40432a84beec8186b99dfdc62851fc6c) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py))

**Принятый API/действие:** POST /api/v1/purchases

**Проверка:** Прогресс не выдаёт права; неизвестный статус отказ, старый оплаченный текст сохранён. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-commerce --test slices_pg d_03_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### D-04 — Д2–Д3 P0

**Сделать:** Пакетная покупка выбранных глав, уникальные позиции, общий расчёт и один commit.

**Главный выходной файл:** `crates/commerce/src/services/batch_checkout.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** checkout_many, purchase_lines

**Сценарий и старый код:** [S035](https://app.notion.com/p/40432a84beec8113b806d4ad851c932b) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_interactions.py))

**Принятый API/действие:** POST /api/v1/purchases/batch

**Проверка:** Ошибка одной строки откатывает всё; повтор не списывает деньги дважды. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-commerce --test slices_pg d_04_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### D-05 — Д3 P0

**Сделать:** Реализовать создание/изменение слоя книг, состав, версии тарифов, полномочия автора и покупку слоя по атомарному D-контракту.

**Главный выходной файл:** `crates/commerce/src/services/layers.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** layers,layer_membership,layer_access

**Сценарий и старый код:** [S031](https://app.notion.com/p/40432a84beec81999c5debde42de045f) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/layers.py)), [S032](https://app.notion.com/p/40432a84beec8165a26ecba9f5922876) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_settings.py))

**Принятый API/действие:** POST /api/v1/layers/{id}/purchase; POST /api/v1/layers; PATCH /api/v1/layers/{id}

**Проверка:** После правки состава прежнее право соответствует согласованной версии. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-commerce --test slices_pg d_05_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### D-06 — Д3–Д4 P0

**Сделать:** Заявка на возврат, состояния, частичный/полный возврат по политике и reversal ledger.

**Главный выходной файл:** `crates/commerce/src/services/refunds.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** refund_requests, ledger reversal

**Сценарий и старый код:** [S036](https://app.notion.com/p/40432a84beec81be8a9cd72973c8f2bd) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/services/purchase_service.py))

**Принятый API/действие:** POST /api/v1/purchases/{id}/refund-request

**Проверка:** Повтор решения не возвращает средства повторно; несуществующая покупка отвергнута. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-commerce --test slices_pg d_06_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### D-07 — Д4 P1

**Сделать:** Обмен покупки с проверкой условий, переносом entitlement без двойного зачисления.

**Главный выходной файл:** `crates/commerce/src/services/exchanges.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** exchange_service, entitlements

**Сценарий и старый код:** [S037](https://app.notion.com/p/40432a84beec81f7968ccdf03bf36a3b) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/services/purchase_service.py))

**Принятый API/действие:** POST /api/v1/purchases/{id}/exchange

**Проверка:** Одновременная покупка/обмен не создаёт двойное право и отрицательный баланс. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-commerce --test slices_pg d_07_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### D-08 — Д4 P1

**Сделать:** Доли автора и команды: снимок версии распределения на покупке, доход и доступ к отчёту.

**Главный выходной файл:** `crates/commerce/src/services/earnings.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** earnings, share_snapshot, settlement

**Сценарий и старый код:** [S038](https://app.notion.com/p/40432a84beec81e7a056e9c123e0f3ab) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/finance.py))

**Принятый API/действие:** POST /api/v1/me/payout-requests

**Проверка:** Поздняя смена долей не переписывает старую проводку. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-commerce --test slices_pg d_08_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### D-09 — Д4 P1

**Сделать:** Выплаты и административные операции: заявка, подтверждение, audit и запрет самоутверждения без права.

**Главный выходной файл:** `crates/commerce/src/services/payouts.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** payout_requests, admin_posting, audit

**Сценарий и старый код:** [S039](https://app.notion.com/p/40432a84beec8139a01fec6b985f61ab) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/finance.py))

**Принятый API/действие:** POST /api/v1/admin/wallet-credits; POST /api/v1/admin/payout-decisions

**Проверка:** Две конкурирующие выплаты не удваивают сумму. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-commerce --test slices_pg d_09_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### D-10 — Д4 P0/блокер

**Сделать:** Внешнее пополнение Boosty: проверить наличие договорного API и достоверного callback; без интеграции пометить S041 blocked.

**Главный выходной файл:** `crates/commerce/src/services/boosty.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** provider_events, verified_receipts; решает I

**Сценарий и старый код:** [S041](https://app.notion.com/p/40432a84beec81ef891ce0d17897a296) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/services/purchase_service.py))

**Принятый API/действие:** POST /api/v1/wallet/topups/boosty (DISABLED, 503)

**Проверка:** Никакой фальшивой оплаты; повтор webhook/неверная подпись не зачисляют дважды. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-commerce --test slices_pg d_10_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### D-11 — Д5 P1

**Сделать:** Реклама/рефералы: финансовый posting по контрактам F, без независимого расчёта денег в F.

**Главный выходной файл:** `crates/commerce/src/services/settlements.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** ledger settlement, referral_rewards

**Сценарий и старый код:** [S038](https://app.notion.com/p/40432a84beec81e7a056e9c123e0f3ab) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/finance.py)), [S039](https://app.notion.com/p/40432a84beec8139a01fec6b985f61ab) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/finance.py))

**Принятый API/действие:** POST /api/v1/me/payout-requests; POST /api/v1/admin/wallet-credits; POST /api/v1/admin/payout-decisions

**Проверка:** Нельзя создать рекламный долг или награду дважды. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-commerce --test slices_pg d_11_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### D-12 — Д5 P0

**Сделать:** Сквозное объединение B/C/H: купил V1 → вышла V2 → повторный вход → читаю V1, деньги и журнал совпадают.

**Главный выходной файл:** `crates/commerce/src/api/mod.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** web wiring I, test/browser/checkout

**Сценарий и старый код:** [S034](https://app.notion.com/p/40432a84beec8186b99dfdc62851fc6c) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py))

**Принятый API/действие:** POST /api/v1/purchases

**Проверка:** PG и браузер: две учётные записи, истечение сессии, отсутствие доступа у другого. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-commerce --test slices_pg d_12_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### D-13 — Д5

**Сделать:** Сверить S031,S034–S039,S041 с текущим NEwBOO и оформить открытые внешние блокеры.

**Главный выходной файл:** `crates/commerce/tests/slices_pg.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** commerce/tests; Notion matrix

**Сценарий и старый код:** [S031](https://app.notion.com/p/40432a84beec81999c5debde42de045f) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/layers.py)), [S034](https://app.notion.com/p/40432a84beec8186b99dfdc62851fc6c) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py)), [S035](https://app.notion.com/p/40432a84beec8113b806d4ad851c932b) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_interactions.py)), [S036](https://app.notion.com/p/40432a84beec81be8a9cd72973c8f2bd) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/services/purchase_service.py)), [S037](https://app.notion.com/p/40432a84beec81f7968ccdf03bf36a3b) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/services/purchase_service.py)), [S038](https://app.notion.com/p/40432a84beec81e7a056e9c123e0f3ab) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/finance.py)), [S039](https://app.notion.com/p/40432a84beec8139a01fec6b985f61ab) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/finance.py)), [S041](https://app.notion.com/p/40432a84beec81ef891ce0d17897a296) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/services/purchase_service.py))

**Принятый API/действие:** POST /api/v1/layers/{id}/purchase; POST /api/v1/purchases; POST /api/v1/purchases/batch; POST /api/v1/purchases/{id}/refund-request; POST /api/v1/purchases/{id}/exchange; POST /api/v1/me/payout-requests; POST /api/v1/admin/wallet-credits; POST /api/v1/admin/payout-decisions; POST /api/v1/wallet/topups/boosty (DISABLED, 503); POST /api/v1/layers; PATCH /api/v1/layers/{id}

**Проверка:** Каждый S имеет ссылку на точный SHA/CI, невыясненное помечено. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-commerce --test slices_pg d_13_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

## Испытания Д6–Д15 и передача I

Д6–Д7: реальная PostgreSQL/миграции/ACL предметной области; Д8–Д9: Chromium и сравнение с NEwBOO; Д10–Д11: конкурентность/падение worker/retry/таймаут; Д12: полный предметный пакет на финальном SHA и интеграция; Д13–Д15: ограниченная бета, аудит исходных S, запись PR/SHA/CI в старую Notion-матрицу. Конкретные опасные сценарии и триггеры — в [tests-and-handoffs.md](../tests-and-handoffs.md).

Для каждого S указать один из статусов: доказан, частично доказан, заблокирован внешней системой, заморожен/исключён по продукту. **Зелёная компиляция — не статус готово.**
