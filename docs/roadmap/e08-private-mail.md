# E-08 / S057 — перенос личной почты NEwBOO, Draft

Основание: [S057](https://app.notion.com/p/40432a84beec816f8449df9a32040f38),
[NEwBOO 17125ef7](https://github.com/HeisLuka/newboo/tree/17125ef758a791e9630505c14d24f7fe65aed228).
Уточнение пользователя 30.10.2026: переносим существующие логику и механику,
не проектируем новую почту. Ни зелёный CI, ни доступность отдельных маршрутов
не подтверждают соответствие источнику. PR #276 возвращён в Draft.

## Сверка источника с TOR

Все пути источника ниже относятся к закреплённому SHA выше и находятся под `backend/`.

| Механика NEwBOO и источник | Состояние TOR в этом черновике |
| --- | --- |
| Кнопка «Почта» в публичном профиле: `app/web/templates/users/profile.html:53`, `view_models/public_profile.py`; POST `/mail/start/{recipient_user_id}` | Исходный URL восстановлен. Придуманные форма номера получателя и предложение обмениваться номерами удалены. Минимальный профиль с действием «Почта» и Chromium click-path добавлен после 6ce2ce4; его CI пока не подтверждён. Полная исходная модель профиля и идентичность имён остаются разрывом. |
| Личный список диалогов, имена участников: `templates/mail/index.html`, `partials/mail/conversation_sidebar.html`, `mail_service._user_view` | Только actor-scoped диалоги; имена загружаются после проверки доступа. Email и password/account rows в HTML не передаются. |
| Сортировка по последней активности, превью из первых 140 символов: `repositories/memory/mail_repository.py:69–101,180–183` | Восстановлены activity ordering, превью и курсор по activity+id. Последний — техническая адаптация к SQL keyset: чужой диалог не может служить источником cursor metadata. |
| Формат времени `%d.%m.%Y %H:%M`, фиксированный MSK UTC+3: `services/time_format_service.py` | Восстановлен для сообщений и списка. UTC timestamp сохраняется отдельно для транспорта. |
| Непустой обрезанный текст, без продуктового ограничения 2000 символов: `services/mail_service.py`, `repositories/memory/mail_repository.py:144–149`, `templates/mail/detail.html` | Удалены `maxlength`, service/DB cap 2000. Сохраняются транспортный body limit и отказ недопустимому для хранения тексту. Нового продуктового лимита не вводим. |
| В просмотренных исходных start/send нет политики 10 диалогов или 30 сообщений в минуту | Оба придуманных ограничения и обслуживающие их индексы удалены. |
| Без client ID одинаковое тело того же отправителя за 0–5 секунд среди последних 20 сообщений возвращает прежнее сообщение: `services/mail_service.py:193–250,724–761`; `tests/contracts/test_mail_service_dedup.py` | Восстановлен fallback для HTML без ключа. ACL и block проверяются до повтора; pair lock сериализует конкурентные отправки. |
| Client ID в HTML/заголовках: фильтр алфавита, до 128 символов, scope conversation+sender, исходный replay: `routes/mail_web_helpers.py`, `repositories/memory/mail_repository.py:155–168` | HTML-протокол теперь имеет отдельный `client_message_id` (заголовок приоритетнее формы, исходная фильтрация/128, повтор по conversation+sender); пустой ID через no-JS использует 5-second fallback. Строгий 43-символьный ключ сохранён только для TOR JSON и совместимого явного legacy POST. Изменения ещё не прошли CI. |
| История: по умолчанию 50, страницы до 100: `services/mail_service.py:896–1013`; старые сообщения подгружаются в поток | Восстановлен верхний предел 100. PG покрывает initial/before/after. В HTML добавлены AJAX prepend и no-JS переход по ссылке. Новая реализация **ожидает браузерной проверки**, а не считается принятым переносом. |
| Polling: видимая вкладка 3 секунды, скрытая 10 секунд без запроса, visibility resume 250 мс: `static/js/mail-detail.js:222–264,372–381` | Восстановлены интервалы. Удалено придуманное отбрасывание карточек после 200 сообщений. Начало с пустого диалога и один запрос в полёте сохранены. |
| В просмотренных mail routes/service/repository/templates/JS нет отдельной кнопки read receipt и mail unread cursor | Придуманные UI, `/mail/{id}/read`, unread DTO, таблица/триггер `social_mail_reads` и service/repository команды удалены. Чтение текста и notification unread — разные вещи. План E-08 уточнён по исходнику. |
| Block через friendship actions: `templates/mail/detail.html`, `social/services/friendship_service.py`, `routes/mail.py` | Двусторонний запрет новых сообщений и независимость блокировок есть. Добавлены POST `/users/{id}/friend`, `/users/{id}/unfriend`, `/users/{id}/block`, состояния pending/accepted, встречное принятие, кнопки в профиле и mail, общий pair-lock c mail. Направленная блокировка остаётся TOR-адаптацией; notify/outbox и полный исходный friendship UX остаются открытыми. Тесты добавлены, CI не подтверждён. |
| Новое сообщение создаёт адресатам notification, повтор не создаёт новую: `services/mail_service.py:762–786` | **Не перенесено**; требуется существующий notification/outbox контракт G, без выдуманного email-провайдера. |
| Исходные book previews, групповые комнаты, admin mail | **Не перенесено**. Для preview необходим исходный ACL книги; закрытый текст/название не резолвится до подтверждения права. Полный модуль почты не закрыт. |

## Разрешённые технические адаптации и открытые разрывы

Утверждённый [TOR JSON API](api-v1.md) требует 43-символьный `Idempotency-Key`,
повтор actor+key+payload и 409 при другом payload. Это основание для JSON facade,
а не разрешение менять UX или незаметно подменять исходный HTML client-message ID.
CRLF/CR→LF нормализуются до сравнения HTML/JSON повторов. Actor берётся только
из сессии; Origin/CSRF и membership проверяются на сервере. Отказы fail closed.

Для первоначального profile→mail browser path, HTML client-ID/retry, AJAX отправки/ошибок/older prepend и friendship actions код и регрессионные тесты теперь присутствуют, **но результат нового CI и независимое ревью отсутствуют**. Для полноты нужны система уведомлений с outbox consumer, ACL previews, исходная модель публичного профиля и её уникальная идентичность.
Группы и admin mail остаются незакрытой частью полного модуля. Это разрывы переноса,
а не новые исключения из продукта. Новые механики или изменения поведения требуют
отдельного явного решения, а не названия «техническое ограничение».

## Продолжение реализации (новые коммиты PR после 6ce2ce4)

В Draft добавлены: публичная страница `GET /users/{display_name}` с POST кнопкой «Почта» (для вошедшего не-самого пользователя, при отсутствии block), реальный profile-click в Chromium, независимая HTML identity `client_message_id` до 128 символов, уникальность `(conversation_id,sender_id,client_message_id)`, запрос исходного receipt под pair lock, AJAX send, явная ошибка с сохранением ID для повтора и AJAX prepend старых сообщений. JSON API сохраняет строгий 43-символьный `Idempotency-Key`; CSRF/Origin и actor scope не ослаблялись. HTML/PG transport теперь расширен дополнительным regression test.

Это **изменения кода без подтверждённого нового CI**. В среде редактирования недоступен локальный Cargo/rustfmt, а для новых SHA нет опубликованных check runs. До зелёных PG18/HTTP/Chromium, fmt/Clippy, RIL/architecture и интеграционного ревью I они не приняты.

**Важные границы:** публичный TOR профиль пока минимален (только имя и действие «Почта»), не содержит всех исходных данных/состояний профиля; при совпадающих display_name из соображений безопасности возвращает 404. Источник не вводил такую политику — нужна отдельная задача восстановления исходной уникальной публичной идентичности. Полные friendship actions, notifications/outbox, book preview ACL, группы и admin mail ещё не перенесены. Не объединять PR до устранения этих разрывов или согласованного изменения scope. Draft migration checksum снова изменён: только на чистых disposable DB, retained ledger проверять отдельно.

## E-06 friendship: код после 6ce2ce4, принятие ожидает CI

Источник: `app/social/services/friendship_service.py:122–243,346–398`, `app/web/routes/users.py:343–420`.
Дополнен Draft SQL `social_friendships`: один pending/accepted на нормализованную пару,
исходящий запрос не принимает сам себя, встречный принимает, запрос друзей
принимается как идемпотентный, remove удаляет; любой block обеих сторон запрещает
запрос и удаляет дружбу внутри общей pair-lock транзакции.
Сохранена отдельная directed-блокировка `social_user_blocks` из TOR.
`crates/web/src/auth/friendship.rs` использует verified session actor, Origin/CSRF,
отклонение подмены формой и относительный safe redirect.
Кнопки представлены в mail и минимальном публичном профиле; тесты PG/HTTP и Chromium добавлены.
Это не доказательство паритета до фактических прогонов. Исходные friendship
notifications, полный профиль и notification inbox ещё не перенесены.

**Уведомления:** в TOR существует общий durable `outbox_events` (миграция
`202610270010_c_chapter_completion_outbox.sql`), но в текущем графе нет
готового обработчика уведомлений по `mail` и `friendship`. Самостоятельную
новую очередь/провайдер в PR не добавляем: без consumer это создало бы
невыполняемые события и ложное ощущение доставки. Требуется G/I интеграция
с исходными правилами dedupe+recipient ACL, затем тест «новое событие 1,
HTML retry 0, блокировка не доставляет». Не считать notification parity закрытым.

## Передача I и проверка

Workspace/Cargo, router/RIL, изменения плана и SQL требуют интеграционного ревью I.
Main route IDs 0–83 сохранены; Draft E-08/E-06 использует 84–95 (12 route pairs).
Миграция `202610300040_e08_private_mail.sql` существует только в PR: в рамках этой
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
имена, polling, replay, third-user denial, block, XSS, ширину и no-JS. Старт в нём
пока выполняется fixture POST на исходный URL: это **не доказательство profile click**.
Прежние 30/30 checks на `09ca74fd` относятся к прежнему поведению и не принимают
новую ревизию или соответствие NEwBOO. Новый результат фиксируется на точном SHA PR.
