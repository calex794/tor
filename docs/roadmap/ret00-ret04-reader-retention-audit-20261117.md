# RET-00 / RET-04 — проверка фактических петель чтения и минимального инвентаря REC

**Источник:** \`calex794/tor\`, \`main@598a3a23de4958e7fa60fbde3d2a5eddef6db5d6\` (17.11.2026). **Статус:** аудит исходного кода и добавленные автоматические тесты; это НЕ акт приёмки работающего VPS и НЕ доказательство роста удержания. Каноническая очередь исполнения — существующий [мастер-план TOR](https://app.notion.com/p/41b32a84beec81cfaeacff801e8a981a). Не создаём второй roadmap/новый сервис.

## RET-00 — что действительно есть в коде

| Читательский переход | Подтверждённые файлы в main | Что проверено и чего доказательство пока НЕ даёт |
| --- | --- | --- |
| Домашняя и публичная витрина | \`crates/web/src/homepage_updates.rs\`, \`crates/search\` | Отдельные \`latest_added\` и \`recent_free_releases\` вызываются, F-04 \`TOR_RISING_SHELF_ENABLED\` по умолчанию OFF. Не доказано, что рабочий VPS настроен так же. |
| Чтение/права/позиция | \`crates/web/src/free_reader_http.rs\`, \`crates/reading/src/services/progress.rs\`, \`crates/reading/src/repositories/progress.rs\` | Free S3 grant после проверки прав; UTF-16 позиция и структурное завершение; \`continue_reading\` берёт разрешённый текущий текст. Продвижение к следующей главе и непрерывность после полного импорта требуют E2E. |
| Личная библиотека | \`crates/reading/src/services/library.rs\`, \`crates/web/src/pages/library.rs\` | Раздельные сохранённые книги, покупки и прогресс; интерфейс «Читаю / Сохранено / Обновления / Куплено», отдельный архив удалённых книг. Полный book-level lifecycle \`reading/later/completed/dropped\` и «вторая meaningful Book» этим не доказаны. |
| Book Follow | \`crates/reading/src/repositories/book_follows.rs\`, \`crates/web/src/book_follow_page.rs\` | Явный Follow, отдельный от Library и от прав чтения; проверка аккаунта/допуска при записи. Есть \`crates/reading/tests/follow_updates_pg.rs\` и \`crates/web/tests/book_follow_http_pg.rs\`. |
| «Хочу продолжение» | \`crates/reading/src/repositories/demand.rs\`, \`crates/reading/tests/continuation_demand_pg.rs\`, \`crates/web/tests/continuation_demand_http_pg.rs\` | Отдельное идемпотентное намерение на released frontier и агрегат \`current_count\`. Не равно Follow, не выдаёт доступ и не означает будущий выпуск. |
| Реальные обновления | \`crates/reading/src/repositories/follow_updates.rs\`, \`crates/web/src/pages/library.rs\`, \`crates/web/src/book_schedule_worker.rs\` | Лента из reader-visible first-release candidates и scheduled worker с отдельным opt-in. Не доказана сквозная каузальная цепь demand → release ID → notification → next meaningful read. |
| Органические REC после главы | \`crates/recommendations/src/repositories/feedback/shelf.rs\`, \`crates/recommendations/src/services/reader_end_shelf.rs\`, \`crates/web/src/recommendation_http/pages.rs\` | SQL строит 3 строго разграниченные категории; Rust ограничивает до 5 разных книг; source Book исключена. Фильтры actor discovery / возраст / скрытие применяются до выдачи. |
| Настройка квот и админка REC | \`crates/recommendations/src/services/reader_end_shelf/policy.rs\`, \`crates/recommendations/src/repositories/feedback/shelf_policy.rs\`, \`crates/web/src/recommendation_http/admin.rs\`, \`crates/web/tests/recommendations_admin_policy_http_pg.rs\` | В **текущем main есть** \`GET/POST /admin/recommendations\`, таблица версионированных квот, CAS, журнал и тесты (часть требует изолированного PG). Исторические документы от 13 ноября с формулировкой «ещё не реализовано» устарели на уровне наличия кода; эксплуатационную готовность не утверждаем. |
| REC флаги/старый tracking | \`crates/web/src/recommendation_http/mod.rs\`, \`docs/contracts/flow-rec-runtime-retirement-v2.md\` | \`TOR_RECOMMENDATIONS_ENABLED=1\` необходим для публичной REC; без него OFF. Индивидуальные analytics/behavioral producers выведены из активной модели, не возвращать их ради Retention. |

### Граница достоверности

Статус «есть файл/маршрут/тест» не равен «работает на VPS», «PG тест запускался на текущем SHA» или «удержание выросло». Нужны отдельные доказательства актуальных CI/PG/Chromium, выборки реальных релизов и допуск к конкретному окружению. Никаких запросов к боевому PostgreSQL в этой ветке.

## RET-04 — минимальный состав, никакой целевой численности авторов

**Нет продуктовой нормы «20–40 владельцев».** Математический минимум зависит от вопроса. **Исходная Book не может рекомендовать себя.** Нужные права и видимость должны быть истинны для каждого рекомендованного кандидата.

SQL классифицирует через \`books.owner_user_id\`:

- **Author:** другая доступная Book *того же* владельца.
- **Teammate:** доступная Book *другого владельца* в активной подтверждённой команде исходной Book.
- **OtherAuthors:** доступная Book владельца *вне* исходного owner и команды. Внутреннее legacy wire-имя \`site\` не означает четвёртый пул.

| Требование для одной исходной Book | Владельцы — минимум | Каталог с учётом исходной Book | Комментарий |
| --- | ---: | ---: | --- |
| Появляется хотя бы одна карточка | 1 | 2 Book одного владельца | Либо 2 владельца с 1 Book у каждого. |
| Полная полка из 5 уникальных карточек без обещания долей | 1 | 6 Book | 1 исходная + 5 допустимых кандидатных из любых разрешённых категорий. |
| Два независимых источника 70/30 | 2 | 3 Book | Source owner имеет ещё 1, другой owner имеет 1; это лишь непустые категории. |
| Три независимых источника 50/40/10 | 3 | 4 Book | Source owner имеет ещё 1, active teammate owner 1, внешний owner 1; это лишь непустые категории. |
| **Полная полка + точные 70/30 в тесте 100 ротаций × 5 органических слотов** | **2** | **7 Book** | Source owner **5 Book включая исходную** (4 кандидата); другой owner **2 Book**. Для текущего allocator это минимальный достаточный состав при постоянных eligible-пулах. |
| **Полная полка + точные 50/40/10 в тесте 100 ротаций × 5 органических слотов** | **3** | **8 Book** | Source owner **5 Book включая исходную** (4 кандидата), active teammate owner **2 Book**, внешний owner **1 Book**. Для текущего allocator это минимальный достаточный состав при постоянных eligible-пулах. |

Примечания:
- Проценты относятся к *серии органических показов*, не одной пятиэлементной полке. Редакционная кампания может резервировать до четырёх мест, поэтому число органических слотов может быть меньше пяти; это другая приёмка.
- Если источник стал пуст после ACL/age/hidden/duplicate-filter, работает policy fallback, а не фиктивный владелец. При явном нуле процента администратором источник запрещён.
- Число исходных владельцев во всём TOR не определяет доступность конкретной полки. Важны eligible-книги и отношения команды **относительно исходной Book**.
- Тест в \`crates/recommendations/tests/ret04_minimum_inventory.rs\` исполняет реальные \`select_shelf\` и default policy, включая недостаточный инвентарь. Это проверка распределителя, а не PostgreSQL-связей Team; последние требуют интеграционных PG-тестов.

## Последующие проверки — только в существующей очереди

| ID | Что сделать | Точное условие готовности |
| --- | --- | --- |
| RET-00a | Сверить реальные env flags и конфигурацию staging после разрешения оператора | Не объявлять READY на основании исходных default flags |
| RET-00b | Проверить единый resume source для Home, Library, главы и release notification | Один PG/Chromium E2E с правами и несколькими вкладками |
| RET-00c | Проверить реальный release → Follow → updates → chapter read | Canonical release ID, отсутствие дубля, корректная ссылка и подтверждённая следующая глава |
| RET-00d | Сверить смысл meaningful read в C/F/FLOW | Развести structural completion и реальное «читатель вернулся»; если требуемая cohort-метрика недоступна — отметить это |
| RET-04a | Проверить минимальные пулы по \`owner_user_id\`, Team и outsider в изолированной PG | Шесть классов инвентаря, включая нулевые, урезанные и полные; права и XSS не изменены |
| RET-04b | Проверить профили редакции 1–4 + 1–4 органических слота и exact-share 100 ротаций | Пять различных Book, ни одного entitlement bypass, fallback под управлением версии долей |

**Первый исполнимый прирост:** тест \`ret04_minimum_inventory.rs\`; его статус подтверждается исключительно точным CI. Следующий пользовательский E2E — RET-00b/c по существующим следам Follow/Library/S058, без новой архитектуры.

Рабочий VPS, платежи, секреты и production migrations не меняются.
