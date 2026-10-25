# TOR — инструкция исполнителю и цена тестирования

Это **не рекомендации**, а последовательность работы. Источник требований — [scenarios.tsv](scenarios.tsv), принимаемые API — [api-v1.md](api-v1.md), PG и события — [data-and-events.md](data-and-events.md), продуктовые инварианты — [decisions-v1.md](decisions-v1.md). Исполнитель не создаёт альтернативных маршрутов, таблиц, статусов и моделей на свой вкус.

## 1. Контроль начала: перед первой строкой Rust

1. `git fetch origin && git checkout main && git pull --ff-only`. Зафиксировать `git rev-parse HEAD`; по названию ветки нельзя судить о свежести. Не работать из старой `lane/*` без актуализации.
2. В [scenarios.tsv](scenarios.tsv) взять назначенные S-коды и реальные исходники `NEwBOO` по SHA; сохранить в PR ссылки. Если старый интерфейс и новая безопасность расходятся, выбрать явно указанное более безопасное поведение `api-v1.md`/`data-and-events.md`.
3. Найти предметного владельца в `architecture/modules.toml`. Не изменять чужой пакет, общие Cargo files, миграции и `web/main` самовольно. Внести согласованный текст миграции и contract change в очередь интегратора I; до принятия интерфейса тестировать свой модуль через fake port.
4. Убедиться, что есть рабочая эталонная fixture PostgreSQL: гость; читатель U1; второй читатель U2; автор U3; команды T1/T2; книга Draft, PublicFree, PublicPaid; глава V1, V2; купленная V1 U1; U2 не купил; revoked purchase. Никогда не выполнять опасные тесты на реальной пользовательской базе.
5. **Один предметный пакет/завершённая пользовательская операция = один PR.** Не делать PR «все модели», «все маршруты» без воспроизводимого результата.

## 2. Точный порядок одной итерации

| Шаг | Команда / действие | Условие перехода |
| --- | --- | --- |
| 1. Начать | `git checkout -b feat/<lane>/<slice>` от актуального main | В PR есть A-xx/Sxxx, source path, целевой контракт |
| 2. Код | `src/domain` → `schemas` → `services` → `repositories` → `api`; отдаются реальные ошибки, не panics | Одна операция работает локально с fake adapter/реальным PG |
| 3. Дешёвые проверки | `python3 tools/check_feature_layout.py`; `cargo fmt --all -- --check`; `cargo check -p tor-<feature> --all-targets` | Не копить некомпилируемый код до конца большого блока |
| 4. Конец предметного блока | `cargo test -p tor-<feature> --all-targets`; PG-тесты конкретной области в CI с `DATABASE_URL` | Все результаты относятся к одному Git SHA, минимальные отрицательные случаи |
| 5. Первая волна ошибок | Скачать **все** проваленные test cases и сгруппировать: compile/schema/auth/SQL/logic/environment. Поправить root cause одним коммитом или пакетом | Повторить только упавшие/непосредственно связанные тесты, не ставить 100 ненужных CI запусков |
| 6. Повтор до зелёного | Повторять target tests; затем **весь обязательный набор области** на финальном SHA | Никакой результат старого SHA не засчитывается |
| 7. Интеграция | PR в TOR, I подключает router/contracts/migration и тестирует B→C→D или B→G→F/E | Composite SHA проверен на настоящих PG/HTTP/browser, main остаётся зелёным |
| 8. Сдача | PR merged SHA, CI run, пользовательский сценарий, Negative/retry test и match к NEwBOO записать **в существующей Notion-матрице** | Только затем «Проверено/Сверено» |

Если нет локального cargo/PG, не подменять тест «всё зелёное»: приложение и PostgreSQL поднимает GitHub Actions; локальный код открывает PR с пометкой "CI pending", не с фиктивной зеленью. На этапе подготовки договора отсутствие Rust-кода в TOR ожидаемо.

## 3. Явная матрица запуска тестов (проект I-07 к реализации)

| Изменились файлы | Быстрый контроль | Тяжёлые проверки после готового блока / в PR | Дополнительно |
| --- | --- | --- | --- |
| `crates/accounts/**` | tor-accounts unit+format+architecture | PG accounts и HTTP auth | CSRF/cookies, соседние B/C/D/E ACL |
| `crates/books/**`, `crates/teams/**` | tor-books/teams | PG publish/versions и browser author | C reader, D pinned edition, G event |
| `crates/reading/**` | tor-reading | PG positions, 501 TOC, browser reader | B/C/D denial, paid progress |
| `crates/commerce/**` | tor-commerce | PG ledger, concurrent duplicate, browser checkout | B version+price и C entitlement; **всегда** |
| `crates/discussions/**`, `social/**`, `clubs/**`, `support/**` | только затронутые feature suites | PG auth/comments/inbox + browser of changed surface | ACL C/B, notifications G |
| `crates/search/**`, `recommendations/**`, `referrals/**` | только затронутые | PG indexes+projection, browser discovery | invalidation B/E/G; D если referral money/ads |
| `crates/ingest/**`, `imports/**` | оба изменённых | PG jobs/checkpoints, URL+archive fuzz, consumer event | publisher B, F projection, E delivery |
| `crates/web/**` или общие templates | tor-web + architecture | chromium routes, HTTP access across affected A–G | если неизвестно — полный browser matrix |
| `crates/contracts/**`, корневой Cargo, migrations, shared router, CI selector | cargo fmt/check workspace, contracts | ВСЕ зависящие PG/HTTP/browser paths, полный обязательный пакет | **не** сужать выбор по имени файла |
| документы `docs/roadmap/**`, строки сценариев | roadmap syntax/coverage/architecture lint | Rust build не нужен, если не менялось Rust/SQL/config | изменение принятого API контракта = broad tests перед внедрением |
| release-кандидат/Д12/Д15 | workspace fmt/check/clippy/test | полные PG + Chromium + отказные/восстановление | точный SHA, не «зелёная ветка вчера» |

**В TOR уже есть** `Repository rules` и `Rust S0 browser preview`; второй сейчас собирает **демо**, а не PG/browsers+auth. I обязан ввести отдельные именные jobs: `fast-boundaries`, `feature-tests`, `postgres-critical`, `browser-e2e`, `release-full`. Меньше jobs допустимо физически, но их обязательный статус/набор тестов нельзя смешивать. `cancel-in-progress` уже применяется к PR — не считать это ещё не выполненной новой оптимизацией.

## 4. Проверяемые команды и инфраструктура

**Для подготовленного кода** (после того, как I добавит crates+PG migrations):

```bash
python3 tools/check_feature_layout.py
python3 -m unittest discover -s tests/architecture -v
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test -p tor-books --all-targets
cargo test -p tor-reading --all-targets
cargo test -p tor-commerce --all-targets
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
```

Для PG тестов I определяет конфигурацию `DATABASE_URL` в выделенном GitHub Actions service `postgres:18`; производственный URL запрещён. Применение миграций до тестов — обязательный шаг job (через SQLx CLI, если установлен и закреплён, либо существующий Rust migration runner, перенесённый из bokarust). **Не утверждать, что эти команды уже прошли TOR с отсутствующими пакетами**. Установить deadline (обычные тесты 10s, тяжёлые integration 60s + отдельный job timeout), создавать в тесте отдельную схему/базу или делать rollback fixtures. При неуспехе сохранять последний успешный артефакт и минимальные логи без секретов.

Браузерный тест — настоящий Chromium на запущенном `tor-web` и PG, не Node mock. Должны быть зафиксированы `HTTP response status`, ответ/URL и доказательство отсутствия закрытого текста. **Известный дефект PR #22**: `page.goto` на 404 падал до чтения статуса, поэтому негативный case проверять через HTTP request context и затем отдельно через страницу; не выключать защиту маршрута.

## 5. Испытательные данные, без неоднозначности

| Fixture | Что создаётся | Кем используется |
| --- | --- | --- |
| `actor_guest` | Нет session, свежий браузер | A,C,F,H |
| `user_reader_a`, `user_reader_b` | Уникальные email/cookie, разные роли и кошельки | A,C,D,E |
| `user_author` | Владеет `book_public`, а не `book_draft_elsewhere` | B,C,H |
| `book_public` | `source_identity=test:public`, `target_language=ru`, 501 глав, из них paid/free, V1,V2 | B,C,D,F |
| `book_draft` | visibility draft + unpublished chapter с уникальным маркером `SECRET_DRAFT_MARKER_DO_NOT_ECHO` | все публичные проверки утечек |
| `book_held` | admin_hold=true + публичная глава | B,C,F,H |
| `wallet_1000` | тестовый RUB баланс через **fixture only**, не публичный topup | D |
| `purchase_v1` | реально проведённое списание U1 и право на V1 | C,D |
| `purchase_revoked` | отдельный purchase+revocation, но история остаётся | C,D |
| `outbox_redelivery` | одно событие в состоянии leased/expired для двух workers | G/E/F |

Случай `book_public` в фикстурах большой: **501 глава**, потому что в bokarust есть ограничение `LIMIT 200`; тест на 20 глав никогда не найдёт этот дефект. Для кошелька использовать суммы в minor units, например 1000 = 10.00 RUB; в выводе проверять обе величины, нельзя спутать рубли и копейки.

## 6. Критерии выпуска (никаких зелёных заглушек)

**Д1:** TOR Rust+PostgreSQL: настоящая книга/публичная глава через HTML/JSON, перезапуск не стирает записи; commit URL, browser screenshot, PG read. RAM-demo не проходит.

**Д5:** каждый включённый сценарий перенесён в код и имеет хотя бы один реальный PG/HTTP путь, нет красного блокирующего P0. Исключения и внешние блокеры не объявлены работающими. 5 дней — цель, не свидетельство исполнения.

**Д12:** полный обязательный пакет (с откатом, idempotency, нагрузкой и безопасностью) на одном SHA, функциональная матрица выверена, PG миграции чистые и upgrade. Red critical → Д13 нельзя запускать.

**Д15:** ограниченная бета со всеми реально допущенными пользователями/ролями; матрица S/F с каждой недоделкой, отдельное решение о публичном выпуске, без ложного 100%.

## 7. Формат передачи следующему исполнителю

```text
TASK: B-06 / S021 S022 S058
BASE: tor/main SHA <actual sha>
SOURCE: newboo SHA 17125ef758; bokarust SHA d11b27b431
OWNERSHIP: crates/books only; shared migration + router I
CONTRACT: POST /api/v1/chapters/{id}/publish; ChapterPublished schema_version=1
DB: chapters + chapter_publications + immutable version + first-release fact + outbox in one commit
TESTS: duplicate command; two publishers; unauthorized author; draft GET 404; browser publication
RESULT: code paths, tests, migrations, PR URL; explain any safe product difference
READY WHEN: green final SHA exact, PG+browser, integrated in tor/main, matrix updated
```

При столкновении двух спецификаций применять: продуктовая конституция/безопасность → текущая одобренная TOR инструкция → историческое поведение старого main → прототип bokarust. Любое отступление оформлять **одним явным diff**, а не 20 вопросов в чате.


## 8. Неразрешённые прежде пограничные случаи — теперь фиксированный договор

**Повтор JSON-команды:** request header `Idempotency-Key` содержит base64url-строку длиной ровно 43, поле `idempotency_key` в JSON теле **не допускается**. Test: missing key -> 422, malformed key -> 422, valid same Actor+key+canonical JSON twice -> original receipt/no duplicate ledger, same key+different amount -> 409, чужой Actor с тем же ключом не получает чужой receipt. HTML формы сохраняют `command_key` и трансформируют его в тот же use-case. SQL запись делается внутри бизнес-транзакции.

**Отдельное событие каждой редакции:** author публикует V1 -> `ChapterPublished(chapter,V1,first_release=true)`, повторяет команду V1 -> **0 дополнительных** events, правит и публикует V2 -> `ChapterPublished(chapter,V2,first_release=false)`, outbox имеет 2 строки с разными `dedupe_key`; бизнес-таблица `chapter_first_release_events` содержит одну. Test на настоящей PG с двумя одновременными публикациями и проверкой всего outbox.

**Fan-out с частичным падением:** `required_consumers = [search-index-v1,subscription-inbox-v1]`. Первый consumer успешно создаёт PG read-model и ack в `event_deliveries`, второй падает до ack; событие остаётся `delivered_at IS NULL`. После retry search-index-v1 **не повторяется**, subscription-inbox-v1 создаёт ровно один inbox entry, после ack событие получает `delivered_at`. Проиграть crash до внешнего ack (потребитель обязан выдерживать replay); проверить `lease_epoch`, чтобы старый worker не пометил чужую попытку завершённой. Это тест предметного контура G/I, не unit mock.

**Условие приёмки:** никакой PR, меняющий публикацию/outbox/checkout, не вливать без этих отрицательных PG-тестов и итогового SHA; собственно наличие markdown-договора не заменяет их выполнения.
