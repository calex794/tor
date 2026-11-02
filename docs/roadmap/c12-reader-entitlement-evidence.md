# C-12: матрица доступа к читалке — S005 / S006 / S007 / S065

**Доказательная база:** TOR `main@f403b3b3` (03.11.2026), [PR #426](https://github.com/calex794/tor/pull/426), [post-merge Integration CI 16/16](https://github.com/calex794/tor/actions/runs/41510027811), [Repository rules](https://github.com/calex794/tor/actions/runs/41510027954). Оригинал NEwBOO — [reader_page.py](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py), [chapter_access_service.py](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/services/chapter_access_service.py).

Здесь перечислены подтверждённые пути и **открытые расхождения**, а не заявление о полном функциональном паритете. Отсутствующий маршрут, возвращающий 404, подтверждает только отсутствие обхода через него; он не доказывает воспроизведение соответствующей функции NEwBOO.

## Сценарии и доказательства TOR

| Сценарий | Реальное доказательство на TOR | Что пока не принято |
| --- | --- | --- |
| [S005](https://app.notion.com/p/40432a84beec8176ad60ff17f805cf20), закрытая платная глава | `crates/web/src/pages/paywall.rs`, `crates/web/tests/paid_grants_http_pg.rs`, `tools/s034_single_paid_browser.cjs`: публичный paywall не содержит платный текст; покупка — отдельная операция с проверкой кошелька/версии | Полный NEwBOO UX объяснения причин закрытия для всех ролей/состояний |
| [S006](https://app.notion.com/p/40432a84beec81bd9802d4e54f687b48), сохранённая позиция | `crates/web/tests/auth_postgres/c06_cases.rs`, `crates/reading/src/repositories/sync.rs`, `tools/paid_reader_minio_chromium_e2e.cjs`: версия V1 закреплена за покупкой, отдельно проверяются прогресс, UTF-16 offset и восстановление | Полная совместная матрица reconnect/offline, Layer CURRENT и обычного купленного V1 |
| [S007](https://app.notion.com/p/40432a84beec814989c1e099bc020a34), личная библиотека | `crates/web/src/auth/library_actions.rs`, `crates/reading/src/repositories/library.rs` и ранее [PR #215](https://github.com/calex794/tor/pull/215): bookmark не создаёт оплаченного права | Сверка всех разновидностей библиотеки и legacy saved-edition NEwBOO |
| [S065](https://app.notion.com/p/40432a84beec8115b1f9f8567f855ce7), отсутствие обхода | `crates/web/tests/paid_grants_http_pg.rs`, `tools/s034_single_paid_browser.cjs`, `crates/web/tests/paid_grants_s3_pg/s034_live_chromium.rs`: U1 V1 разрешён, чужой Actor/отзыв/hold/delete запрещены, реальная приватная PNG идёт MinIO → WebCrypto в Chromium desktop/mobile | Единственная проверенная на одном SHA матрица всех ролей, Layer, legacy history/preview/CDN, SEO/search/ads и cache invalidation |

## Матрица доступа — фактически проверенные границы

| Контекст / состояние | TOR | Где проверено |
| --- | --- | --- |
| Гость видит public/paid | Paywall, но не тело или signed URL | S005 browser; `paid_grants_http_pg.rs` |
| Пользователь с прямой покупкой V1 | Купленная V1, включая приватную PNG, независимо от последующей V2 | S034 real PG/MinIO/Chromium, PR #426 |
| Посторонняя учётная запись или автор без покупки | Нельзя использовать чужой purchase_id как право на V1/asset/progress | `paid_grants_http_pg.rs`; S034 browser |
| Creator draft/withdrawal, оплаченная ранее V1 | Действующая pinned V1 остаётся читаемой при отсутствии hard hold | S034 browser, D entitlement |
| Book/chapter admin_hold | Отказ даже ранее купившему, без новых grants | S034 browser, PG |
| Book/chapter is_deleted | Не возвращается платный текст, paid page 404 | S034 browser, PG |
| Purchase revoked | Новый платный grant, asset grant и paid reader запрещены | S034 browser; S034 PG |
| Чужая запись прогресса | Не создаёт права, нельзя читать чужую позицию | `paid_grants_http_pg.rs`, `s034_single_paid_browser.cjs` |
| History/preview/CDN/media alias | Некоторые исторические пути отсутствуют и отвечают 404 | S065/C-12 negative route matrix; **не** эквивалент полной legacy-функции |

## Ключевое различие NEwBOO и TOR — не сливать модели авторизации

В исходном [NEwBOO ChapterReaderGate](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/services/chapter_access_service.py) `can_open_reader` учитывает `can_bypass_moderation` (moderator/admin/право управлять книгой), `is_publicly_readable` и `has_library_access` для сохранённой версии. В TOR [решение D](../../crates/commerce/src/services/entitlements.rs) требует реальной покупки для обычного paid reader; право автора на preview должно быть **отдельным** маршрутом, а bookmark не заменяет entitlement. Это намеренное более строгое правило по [ACL-контракту](access-authorization-contract.md), а не основание переносить legacy bypass.

## Приоритет оставшейся C-12 приёмки

1. Завершить отдельную матрицу Layer CURRENT (lifetime / 30d / expired / hidden / blocked) после принятия [Draft PR #431](https://github.com/calex794/tor/pull/431). Не считать Draft подтверждённой реализацией.
2. Сверить active и отказные поверхности author preview, moderator/admin и private/coauthor **по их собственным маршрутам**, не подменяя direct buyer grant. Явно отметить расхождение со старыми правами NEwBOO.
3. Отдельно проверить SEO/search/ads/comments/notification, CDN/S3 bearer TTL и инвалидацию cache при hold/revoke. Уже выданный signed URL живёт до истечения TTL; мгновенного отзыва скачанных байтов нет.
4. Для не реализованных history/preview/CDN routes определить: портировать закрытую функциональность с общей политикой или исключить из продуктового паритета. Существующий 404 — безопасная граница, но не готовая возможность.
5. Только после общей actor × state × surface матрицы с PostgreSQL + Chromium, сквозного CI и сравнения с NEwBOO менять «S065 частично» на «принят». Production и деплой вне этой приёмки.

**Текущее заключение:** PR #426 успешно принят на `main@591a456f`, но весь S065 и C-12 ещё не приняты; счётчик 20/60 из-за этого среза не увеличивать.
