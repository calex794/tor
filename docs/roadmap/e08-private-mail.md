# E-08 / S057 — перенос личной почты NEwBOO, Draft

Основание: [S057](https://app.notion.com/p/40432a84beec816f8449df9a32040f38),
[NEwBOO 17125ef7](https://github.com/HeisLuka/newboo/tree/17125ef758a791e9630505c14d24f7fe65aed228).
Уточнение пользователя 30.10.2026: переносим существующие логику и механику,
не проектируем новую почту. Ни зелёный CI, ни доступность отдельных маршрутов
не подтверждают соответствие источнику. PR #276 возвращён в Draft.

## Интеграционный контроль после I-06 (срез main@b48cb7b2, 31.10.2026)

- Main уже включает P0/I-06 и проверенный на exact SHA read-only ruleset preflight; этот статус **не** распространяется на Draft E-08.
- E-08@85e6c0e7 отстаёт от main на 36 коммитов. Совпадают 13 изменённых файлов с main и 17 файлов с Draft E-11; маршрутные ID 112–117 заняты в main и не могут быть перезаписаны E-08. Нужна явная совмещённая RIL таблица с уникальными ID и обновлёнными classifier, governance, JSON и архитектурными тестами.
- Код P0 public-book-preview на 85e6c0e7 не имеет GitHub check-runs. Предыдущий SUCCESS@8499082f не доказывает поведение новых превью. PR остаётся Draft, merge запрещён до полной проверки на финальном SHA.
- E-08 SQLx миграции перенумерованы **без изменения SQL-байтов** с `202610300040/041` на `202610310200/201` (после текущей максимальной migration main `202610310181`). При новом сдвиге main порядок следует проверить снова. Retained DB migration ledger/checksums не проверены и никакие миграции не выполнялись.
- Открыты функциональные разрывы: исходные notification preferences/tabs, идентичность публичного профиля, private/legacy book previews, group/admin mail. Зелёный CI не приравнивается к полному исходному паритету.

## Актуальный статус (Draft PR #276)

Приёмочный baseline личной почты: `3e6c8107`, полный CI SUCCESS.
На `35f6d99f` PostgreSQL runtime, migrations, reader-browser и HTTP load
прошли; общий CI остановился на единственном Clippy-предупреждении в тесте,
исправленном коммитом `8806675d`. Обновлённая проверка Chromium уведомлений
и истории вошла в ветку. После синхронизации с `main` `7056869c` изменения
проходят повторную приёмку. PR остаётся Draft, без независимого ревью и без
изменений `main`, VPS или retained DB.

## Сверка источника с TOR

Все пути источника ниже относятся к закреплённому SHA выше и находятся под `backend/`.

| Механика NEwBOO и источник | Состояние TOR в этом черновике |
| --- | --- |
| Кнопка «Почта» в публичном профиле: `app/web/templates/users/profile.html:53`, `view_models/public_profile.py`; POST `/mail/start/{recipient_user_id}` | Исходный URL восстановлен. Придуманные форма номера получателя и предложение обмениваться номерами удалены. Минимальный профиль с действием «Почта» и Chromium click-path добавлен после 6ce2ce4; Chromium profile-click в базовом `3e6c8107` подтверждён. Полная исходная модель профиля и идентичность имён остаются разрывом. |
| Личный список диалогов, имена участников: `templates/mail/index.html`, `partials/mail/conversation_sidebar.html`, `mail_service._user_view` | Только actor-scoped диалоги; имена загружаются после проверки доступа. Email и password/account rows в HTML не передаются. |
| Сортировка по последней активности, превью из первых 140 символов: `repositories/memory/mail_repository.py:69–101,180–183` | Восстановлены activity ordering, превью и курсор по activity+id. Последний — техническая адаптация к SQL keyset: чужой диалог не может служить источником cursor metadata. |
| Формат времени `%d.%m.%Y %H:%M`, фиксированный MSK UTC+3: `services/time_format_service.py` | Восстановлен для сообщений и списка. UTC timestamp сохраняется отдельно для транспорта. |
| Непустой обрезанный текст, без продуктового ограничения 2000 символов: `services/mail_service.py`, `repositories/memory/mail_repository.py:144–149`, `templates/mail/detail.html` | Удалены `maxlength`, service/DB cap 2000. Сохраняются транспортный body limit и отказ недопустимому для хранения тексту. Нового продуктового лимита не вводим. |
| В просмотренных исходных start/send нет политики 10 диалогов или 30 сообщений в минуту | Оба придуманных ограничения и обслуживающие их индексы удалены. |
| Без client ID одинаковое тело того же отправителя за 0–5 секунд среди последних 20 сообщений возвращает прежнее сообщение: `services/mail_service.py:193–250,724–761`; `tests/contracts/test_mail_service_dedup.py` | Восстановлен fallback для HTML без ключа. ACL и block проверяются до повтора; pair lock сериализует конкурентные отправки. |
| Client ID в HTML/заголовках: фильтр алфавита, до 128 символов, scope conversation+sender, исходный replay: `routes/mail_web_helpers.py`, `repositories/memory/mail_repository.py:155–168` | HTML-протокол теперь имеет отдельный `client_message_id` (заголовок приоритетнее формы, исходная фильтрация/128, повтор по conversation+sender); пустой ID через no-JS использует 5-second fallback. Строгий 43-символьный ключ сохранён только для TOR JSON и совместимого явного legacy POST. Исходная HTML identity подтверждена CI на `3e6c8107`; новый уведомительный срез ожидает отдельной приёмки. |
| История: по умолчанию 50, страницы до 100: `services/mail_service.py:896–1013`; старые сообщения подгружаются в поток | Восстановлен верхний предел 100. PG покрывает initial/before/after. В HTML добавлены AJAX prepend и no-JS переход по ссылке. Проверка 55 сообщений и AJAX prepend добавлена в Chromium на `6651cf23`; требуется успешный повторный CI на объединённой ветке. |
| Polling: видимая вкладка 3 секунды, скрытая 10 секунд без запроса, visibility resume 250 мс: `static/js/mail-detail.js:222–264,372–381` | Восстановлены интервалы. Удалено придуманное отбрасывание карточек после 200 сообщений. Начало с пустого диалога и один запрос в полёте сохранены. |
| В просмотренных mail routes/service/repository/templates/JS нет отдельной кнопки read receipt и mail unread cursor | Придуманные UI, `/mail/{id}/read`, unread DTO, таблица/триггер `social_mail_reads` и service/repository команды удалены. Чтение текста и notification unread — разные вещи. План E-08 уточнён по исходнику. |
| Block через friendship actions: `templates/mail/detail.html`, `social/services/friendship_service.py`, `routes/mail.py` | Двусторонний запрет новых сообщений и независимость блокировок есть. Добавлены POST `/users/{id}/friend`, `/users/{id}/unfriend`, `/users/{id}/block`, состояния pending/accepted, встречное принятие, кнопки в профиле и mail, общий pair-lock c mail. Направленная блокировка остаётся TOR-адаптацией; notify/outbox реализованы в Draft отдельным срезом, их интеграционный CI ещё проверяется; полный исходный friendship UX остаётся открытым. |
| Новое сообщение создаёт адресатам notification, повтор не создаёт новую: `services/mail_service.py:762–786` | В Draft добавлены source event в общей `outbox_events` внутри транзакции отправки, idempotent `event_deliveries` projection, закрытая `/notifications` и отметки о прочтении; ручных mail read receipts нет. PostgreSQL runtime предыдущего среза прошёл, Chromium notification check нового среза ожидает CI. Полные исходные preferences/tabs не перенесены. |
| Исходные book previews, групповые комнаты, admin mail | **Частично в следующем P0-срезе, CI ещё не подтверждён:** канонические numeric book IDs через B public catalog; HTML, AJAX receipt, JSON poll и older используют общую проекцию. Draft/held/deleted остаются plain text. Legacy aliases, actor-authorized private previews, group/admin mail ещё не перенесены. |

## Разрешённые технические адаптации и открытые разрывы

Утверждённый [TOR JSON API](api-v1.md) требует 43-символьный `Idempotency-Key`,
повтор actor+key+payload и 409 при другом payload. Это основание для JSON facade,
а не разрешение менять UX или незаметно подменять исходный HTML client-message ID.
CRLF/CR→LF нормализуются до сравнения HTML/JSON повторов. Actor берётся только
из сессии; Origin/CSRF и membership проверяются на сервере. Отказы fail closed.

Для первоначального profile→mail browser path, HTML client-ID/retry, AJAX отправки/ошибок/older prepend и friendship actions код и регрессионные тесты теперь присутствуют, **но результат нового CI и независимое ревью отсутствуют**. Для полноты нужны исходные настройки и вкладки уведомлений (producer/consumer уже в Draft), ACL previews, исходная модель публичного профиля и её уникальная идентичность.
Группы и admin mail остаются незакрытой частью полного модуля. Это разрывы переноса,
а не новые исключения из продукта. Новые механики или изменения поведения требуют
отдельного явного решения, а не названия «техническое ограничение».

## Продолжение реализации (новые коммиты PR после 6ce2ce4)

В Draft добавлены: публичная страница `GET /users/{display_name}` с POST кнопкой «Почта» (для вошедшего не-самого пользователя, при отсутствии block), реальный profile-click в Chromium, независимая HTML identity `client_message_id` до 128 символов, уникальность `(conversation_id,sender_id,client_message_id)`, запрос исходного receipt под pair lock, AJAX send, явная ошибка с сохранением ID для повтора и AJAX prepend старых сообщений. JSON API сохраняет строгий 43-символьный `Idempotency-Key`; CSRF/Origin и actor scope не ослаблялись. HTML/PG transport теперь расширен дополнительным regression test.

Базовый срез почты `3e6c8107` прошёл полный CI, но это не приёмка новых уведомлений и более поздней матрицы. Новые Rust/SQL/Chromium изменения проверяются отдельным CI на HEAD PR; без него функциональный паритет не заявляется.

**Важные границы:** публичный TOR профиль пока минимален (только имя и действие «Почта»), не содержит всех исходных данных/состояний профиля; при совпадающих display_name из соображений безопасности возвращает 404. Источник не вводил такую политику — нужна отдельная задача восстановления исходной уникальной публичной идентичности. Часть friendship actions и on-site notifications/outbox реализованы в Draft, но не закрыты по полному исходному UX; book preview ACL, группы и admin mail ещё не перенесены. Не объединять PR до устранения этих разрывов или согласованного изменения scope. Draft migration checksum снова изменён: только на чистых disposable DB, retained ledger проверять отдельно.

## E-06 friendship: код после 6ce2ce4, принятие ожидает CI

Источник: `app/social/services/friendship_service.py:122–243,346–398`, `app/web/routes/users.py:343–420`.
Дополнен Draft SQL `social_friendships`: один pending/accepted на нормализованную пару,
исходящий запрос не принимает сам себя, встречный принимает, запрос друзей
принимается как идемпотентный, remove удаляет; любой block обеих сторон запрещает
запрос и удаляет дружбу внутри общей pair-lock транзакции.
Сохранена отдельная directed-блокировка `social_user_blocks` из TOR.
`crates/web/src/auth/friendship.rs` использует verified session actor, Origin/CSRF,
отклонение подмены формой и относительный safe redirect.
Кнопки представлены в mail и минимальном публичном профиле; тесты PG/HTTP и Chromium добавлены. Исходный start из заблокированного публичного профиля теперь отвечает 303 на `/mail?error=blocked` и показывает сообщение, вместо сухого 403. У имени профиля добавлен индекс `users_display_name_lookup`; дубли имён всё ещё дают fail-closed 404.
Это не доказательство паритета до фактических прогонов. Для исходных friendship notifications и notification inbox добавлен Draft-срез; полный профиль и все исходные настройки уведомлений ещё не перенесены.

**Уведомления:** общий `outbox_events` остаётся единым I/G контрактом
(миграция `202610270010_c_chapter_completion_outbox.sql`).
События mail-created и friendship-request/accept/remove записываются атомарно с
изменением источника; отдельный обработчик выполняет idempotent PG projection в
`user_notifications` и `event_deliveries`, проверяя actor/recipient, membership,
блокировки и согласованную pair-lock. Веб-интерфейс `/notifications` поддерживает
только actor-scoped feed и CSRF-отметки; содержимое сообщений не копируется.
Добавлены PostgreSQL/HTTP тесты повтора, чужого прочтения, block/send и браузерный
тест нового события на 390/1440. Интеграционный CI нового HEAD и ревью G/I
необходимо завершить; исходные preferences/full tabs остаются задачами.

## Передача I и проверка

Workspace/Cargo, router/RIL, изменения плана и SQL требуют интеграционного ревью I.
IDs маршрутов актуального при объединении `main` 0–109 сохранены; E-08/E-06 и notifications добавлены на 110–124 (15 route pairs), без перенумерации опубликованных маршрутов.
Миграции `202610310200_e08_private_mail.sql` и `202610310201_e08_user_notifications.sql` существуют только в PR: в рамках этой
работы применялась лишь к disposable CI PG. Её draft checksum изменён при удалении
придуманных ограничений/read receipts; миграции из main не менялись. Новый CI должен
создать чистую БД. Перед merge отдельно сверить candidate graph и установленный
ledger; не переносить изменённый checksum в retained DB с прежней записью.
Main и VPS этой корректировкой не меняются.

Проверки:

```bash
cargo test -p tor-social --lib
cargo test -p tor-social --test slices_pg e_08_ -- --ignored --test-threads=1
cargo test -p tor-web --test mail_http_pg e_08_ -- --ignored --test-threads=1
python3 -m unittest discover -s tests/architecture -q
```

PG-проверки включают participant ACL, concurrent replay, block/send race, reconnect,
страницы 50/100, отправку более 30 сообщений и текста длиннее 2000, исходный 5-second
fallback и сортировку/превью по активности. Browser 390/1440 проверяет transport,
имена, polling, replay, third-user denial, block, XSS, ширину и no-JS. Старт выполняется реальным нажатием кнопки профиля без fixture POST. Новый browser test
также проверяет 55 сообщений, AJAX older prepend и отдельные уведомления адресата;
на объединённом HEAD эти дополнительные проверки ещё ожидают CI.
Прежние 30/30 checks на `09ca74fd` относятся к прежнему поведению и не принимают
новую ревизию или соответствие NEwBOO. Новый результат фиксируется на точном SHA PR.

## P0: public book previews after 8499082f (CI pending)

Source: NEwBOO mail_book_previews.py, 3 refs/message, 40 refs/page. Unscoped include_hidden=true fallback is forbidden. TOR resolves canonical numeric IDs only via B-owned GetPublicBook, requiring public visibility and rejecting moderation hold/deleted. Cover URL requires B-owned public cover slot. Unresolved links remain plaintext. HTML, AJAX receipt, JSON polling and older pagination use one projection. Tests cover parser/escaping, PG public/draft/held, AJAX and third actor. Slug/legacy aliases, per-actor private book ACL, group/admin mail, public profile identity and preferences remain unimplemented. Retained ledger check, CI on final SHA and independent review remain release gates. Draft must not merge.


### E-08 P0 parser hardening (follow-up; CI pending)

Bound preview parsing to at most three recognized book-link occurrences per message (including duplicates), cap each preceding-token scan to 256 Unicode characters, and preserve whitespace surrounding resolved links. Omit unsupported display_url aliases from the JSON DTO. Client-side book URL and cover URL must match the same canonical resolved book ID. Added parser regression tests. These changes require Rustfmt, Clippy, PostgreSQL/Chromium acceptance on the exact final commit; this note does not claim a completed run.

## 31.10.2026 — partial notification tabs; source preferences remain open

The source `GET /notifications?tab=notifications|discussions|releases|system` separates
release kinds (`chapter_created`, `project_news`, `author_new_book/layer/collection`)
from system notifications and excludes comment/discussion kinds from the system tab.

This Draft slice adds server-side PostgreSQL filtering **before cursor LIMIT** for
`notifications`, `releases` and `system`, with bounded actor-scoped results and
preserved `before` pagination. The `discussions` tab delegates to the existing
E-03 actor-scoped `/me/inbox`; this does **not** claim full source discussion UX.
The invalid/unknown tab falls back to the all-notifications view. The existing
mark-read and mark-all CSRF/recipient guards are unchanged.

The isolated PostgreSQL/HTTP E-08 acceptance tests now cover mail appearing in
`system` but not `releases`, invalid-tab fallback and discussion redirect. This
change requires its **own exact-HEAD CI** after publication; earlier 17/17 success
at `de852dc1` and ongoing checks at `a1e62b01` do not validate it.

**Still blocked:** source per-kind notification preferences/persistence, full
discussion filters, actor-authorized private/legacy book previews, complete
public profile identity, group/admin mail, independent review, retained SQLx
ledger/checksum. No production/SSH/VPS/retained migrations.

## 31.10.2026 — category notification preferences (Draft; verify exact-head CI)

Source NEwBOO @17125ef7 has **22** independent bell categories, keyed by
`(user_id,kind)`, with missing saved rows treated as enabled. A POST to
`/notifications/preferences` saves the entire checkbox set, including disabled
(unticked) categories. This is not the separate content-notification delivery
mode already implemented in TOR's reader settings.

This Draft slice introduces an independent PostgreSQL migration
`202610310300_e08_notification_preferences.sql` (above the current main
migration maximum `202610310291`). Its actor-bound repository methods
return all 22 categories with default-enabled flags and atomically upsert
the full selection. A new private, CSRF/Origin-guarded POST accepts checkbox
names `pref_<kind>` (rather than a repeated key), ignores unknown keys
and never accepts recipient/user identity from the form. The page presents
the original category labels and descriptions, and retains all existing
notification tabs and read/unread behavior.

The existing idempotent social outbox projector checks
`user_notification_preferences` **before** its insertion into
`user_notifications`. Disabled kinds produce no new notification, while
their source event and `event_deliveries` receipt remain intact. Existing
notification rows are not retroactively deleted or hidden. PostgreSQL and
HTTP regressions cover default-enabled preference values, persistent
choices, recipient isolation, CSRF/Origin denial, disabled mail/friendship
projection, and receipts. Future notification producers must share this
policy; the currently integrated producer kinds are only `mail` and
`friendship`, so not all 22 kinds have end-to-end producer parity.

**Acceptance gate:** full CI and architecture regression checks on the
exact commit published with this slice; previous green `2871609a`
does not validate it. Retained SQLx ledger/checksum, independent review,
public-profile identity, private/legacy book previews, full discussions,
group/admin mail and remaining source parity are still open. No
main/production/VPS/retained DB change is authorized.

## 01.11.2026 — public profile identity hardening (next Draft slice)

Source NEwBOO `app/repositories/user_repository.py` folds display-name keys
using `strip().lower()` and refuses duplicate new memory users. Current TOR
SQL accounts allow duplicate display names, including names differing only in
case. Exact-case lookup lets the same apparent nickname lead to different
accounts. A blind unique migration would fail for existing duplicates.

The next scoped change folds public-name lookup using PostgreSQL
`lower(display_name)=lower($1)` after input trimming; it resolves exactly
one matching account, or returns None (HTTP 404) for missing/ambiguous keys.
An append-only nonunique expression index bounds lookup cost without deleting
or renaming users. Real PostgreSQL and authenticated/guest HTTP regressions
check case-normalized success, duplicate 404, and continued distinct account
ownership. Later identity migration must handle historical collisions and
explicit stable handles; this is **not** full source-profile parity. Private
book previews and group/admin mail also remain open. No privileges or user
identifiers come from request text; no production or retained data changes.
 
## 01.11.2026 — notification Open/read navigation (Draft, pending exact-HEAD CI)

The pinned source `/notifications/{id}/read` POST marks the authenticated
recipient's notification as read and then redirects to the form's validated
same-origin `next` target. TOR previously acknowledged and always returned
to `/notifications`, while title links navigated without marking read.

This Draft slice renders an actor/CSRF-guarded `Открыть` POST for both read
and unread rows, including the server-stored event URL as a hidden `next`
value. The read handler preserves `mark_read(actor, id)` and rejects external,
scheme-relative, backslash, control-character, API and dot-segment targets,
falling back to `/notifications`; absent `next` remains backward compatible.
No new endpoint or SQL migration is introduced. Unit and disposable HTTP
regressions exercise safe and hostile redirects. **Do not accept the slice
without exact-HEAD CI, retained migration review and independent review.**

## 01.11.2026 — explicit legacy mail Book aliases (Draft slice)

Source `mail_book_previews.py` accepts book IDs, public IDs and slugs and
also tries an unsafe `list_all_books_async(include_hidden=True)` fallback.
TOR's SEO URL contract has **canonical numeric `/books/{id}` only** and
the B Book schema stores `(source_identity,target_language)`, **not**
source `public_id` / `slug`. Automatically guessing a mapping is incorrect
and may expose hidden works or remap another translated Book.

This slice introduces an **explicit opt-in alias mapping** in new migration
`202611010020_e08_mail_book_legacy_refs.sql`, keyed by one unique
lowercase ASCII alias (1–128 chars, must begin with letter). It is empty
until a separately reviewed, one-to-one NEwBOO import supplies mappings;
there is deliberately no mass legacy backfill or new HTTP slug route.
The only output URL remains the canonical `/books/{id}` for public works
or the existing `/me/books/{id}` for a verified owner. Unsupported
Unicode/ambiguous numeric aliases stay literal text.

The B repository resolves at most 40 requested aliases using a single
actor-bound PostgreSQL query. Hidden drafts require matching
`books.owner_user_id` **and** an active `book_members` owner role;
`admin_hold` and `is_deleted` deny all aliases. E's mail enrichment then
passes canonical IDs through the existing public/owned Book methods.
A failed lookup/DB outage never falls back to an unscoped catalogue scan.
Alias and numeric refs share the original 3-per-message and 40-per-page
bounds. No private covers are emitted. Existing `/book(s)/{numeric_id}`
and notification/mail behavior is unchanged.

Real `mail_http_pg` regressions check public and private aliases across
the sender and other mail participant, denied held/unmapped aliases,
duplicate mapping rejection and a post-send `admin_hold` transition.
B-10 correctly forbids deactivating the owner while their 100% current
revenue share remains assigned; the HTTP test must not bypass that fence.
A separate, valid revenue-reassignment/owner-revocation scenario is still
needed to prove the membership-specific ACL transition. Unit tests
cover bounded parsing, rejected numeric ambiguity and canonical output.
The data migration **does not populate actual NEwBOO aliases**; this is
infrastructure and proof of safe behavior for imported mappings, not full
legacy identity parity. Full source alias import, collaborator-authorized
destinations, discussions and group/admin mail remain open. Exact-head
CI + SQLx ledger review are still acceptance gates. No main, production,
SSH, VPS or retained database operations.

## 01.11.2026 — E-08 group-mail repository foundation (Draft slice)

Source `mail_service.start_group_conversation` creates a titled group
with the authenticated creator plus deduplicated members (minimum two).
Its `mail_repository` keeps group membership status independently
of direct conversation pairs. TOR currently stores only two-person
`social_conversations` with immutable `user_low/user_high` and
therefore cannot safely pretend that a direct conversation is a group.

This isolated Draft slice adds new migration
`202611010030_e08_group_mail.sql` and the E-owned
`PgGroupMail` repository. Direct pair data, IDs, actor ACL and existing
routes remain unchanged. The new group aggregate provides atomic
creator+member creation, member-scoped bounded group listing and message
pagination, 43-character command-key replay fencing, actor-bound sends
and member leave. A database trigger also denies inserts from nonmembers
and mutation of persisted message bodies. Creation reuses direct-mail pair
locks and refuses group membership when creator and participant have blocked
one another. Every send and member exit locks the same membership row
to prevent a stale check-and-send race.

This is **backend scaffolding**, not yet a public group UX: no new
HTTP endpoints, HTML page, invite/remove/ban/rejoin commands, notification
producer, or admin mail delegation is introduced. Creator exit remains
disabled until an explicit ownership-transfer rule is agreed. The
32-member/120-character bounds are TOR operating caps and require
explicit comparison with source behavior; group membership is not
silently derived from friendship, a mail peer ID, or a request header.
Isolated PostgreSQL regressions under the existing
`e_08_` CI filter test creation, bidirectional block checks, membership
authorization, cross-group key conflicts, immutable messages, leave/revoke,
a concurrent send versus member exit, and direct-mail separation.
The disposable installed-ledger upgrade contract now expects six E-08
migrations while retaining the original 67 main SQLx blobs/checksums.

Acceptance requires full exact-HEAD CI including group PG regression,
Repository rules, HTTP load, retained SQLx ledger review, independent
review and remaining S057 source parity. No migration runs on retained
data, no main merge, no VPS/production actions.

 
## 01.11.2026 — групповой интерфейс на основе PgGroupMail (Draft)

Подготовлены веб-страницы `GET /mail/groups`, `POST /mail/groups`, `GET /mail/groups/{id}`, `POST /mail/groups/{id}/messages`, `POST /mail/groups/{id}/leave`, только с серверной проверкой сеанса, Origin/CSRF и активного участия. Создание с выбором двух собеседников из текущей личной почты (не отдельный поиск пользователей), история и страницы сообщений по 50, отправка с повторяемым ключом без дублей, выход для обычного участника. Названия и сообщения выводятся как экранированный текст, скрипты на страницах запрещены. Пять новых RIL-маршрутов с явным доказательством scoped ACL; PostgreSQL/HTTP тест проверяет роль создателя, постороннего, вышедшего, повторную отправку, CSRF, отображение текста.

Это первый интерфейс, **не полный S057**: приглашения в готовую группу, повторный вход, внешние уведомления, административные диалоги и сверка с полным UX NEwBOO остаются отдельно. Не считать принятым без полного CI на точном commit SHA. main / рабочий PostgreSQL / VPS не затрагивать.

 
## 01.11.2026 — E-08: on-site group-mail notification producer and consumer

Каждое новое групповое сообщение создаёт отдельное событие `social.group_mail.created` для каждого активного участника, кроме отправителя, **в той же транзакции PostgreSQL**, что и сообщение. Повторная отправка с тем же `command_key` не создаёт новых событий. Существующая очередь `user_notifications` преобразует подтверждённые события в личные уведомления категории `mail` со ссылкой `/mail/groups/{id}`, без текста переписки в уведомлении. При обработке повторно проверяются блокировка между автором и получателем, действующее участие получателя (с блокировкой строки от одновременного выхода), подлинность исходного сообщения и настройка `mail`. Выход до обработки подавляет ожидающее уведомление. Добавлен регрессионный PostgreSQL-сценарий повторов, двух участников, выхода, блокировки и отключения `mail`.

**Статус: изменения рабочей ветки, не приёмка.** Полный exact-SHA CI, PostgreSQL и пользовательские сценарии должны быть успешны до готовности PR. Не касается живой БД, серверов и основной ветки.


## 01.11.2026 — E-08: создание группы с одним приглашённым

По исходному `mail_service.start_group_conversation` в NEwBOO группа может состоять из автора и одного приглашённого (`len(clean_member_ids) >= 2`). В TOR серверный `PgGroupMail::create` уже допускает такой состав, но HTML раньше требовал двух собеседников. Исправлены форма и `POST /mail/groups`: первый собеседник обязателен, второй необязателен; пустое значение и отсутствие поля равнозначны, некорректный числовой ID и дубликат возвращают 422. Действуют прежние CSRF, Origin, проверка блокировок и членства.

Добавлен HTTP/PostgreSQL сценарий на создание пары, доступ приглашённого и запрет третьему пользователю, а также отрицательная проверка ID. Это только изменение Draft PR №276, новая приёмка GitHub требуется после коммита.
