# E-08 / S057 — личная переписка: первый полный пользовательский срез

Источник: [NEwBOO mail.py](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/mail.py),
`mail_service.start_conversation_async`, `send_conversation_message_async`;
[S057](https://app.notion.com/p/40432a84beec816f8449df9a32040f38).
Это реализация TOR, не утверждение полного паритета исходного модуля.

## Поведение и границы

- Два вошедших пользователя, один диалог на неупорядоченную пару. Пользователь
  начинает диалог по номеру, который собеседник видит в своей `/mail`.
  Профильные ссылки/адресная книга и публичные имена требуют отдельного A/E контракта;
  feature не читает приватные account rows, email или настройки чужого профиля.
- Текст сообщения и его identity неизменяемы; actor только из PostgreSQL-сессии.
  HTML и JSON используют один `MailService` и `PgMail`, не два хранилища.
- Повтор actor+command_key+conversation+body возвращает тот же Message ID;
  тот же ключ с другим телом или диалогом даёт 409. Регистрируется один insert,
  не эвристическое удаление одинаковых сообщений за несколько секунд.
- Сообщения читают только оба участника. Чужой и отсутствующий диалог — 404
  без имени, текста, списка участников или сведений о книге.
- Блокировка с любой стороны запрещает start/send в обоих направлениях;
  снять можно только собственную блокировку. Ранее доступная история остаётся.
  Проверка и запись сериализованы общим PostgreSQL pair advisory lock.
- Курсор прочитанного двигается только вперёд и относится к реально существующему
  сообщению данного диалога. GET/poll не отмечает сообщение прочитанным.
- Plain text, экранирование HTML; ссылки на книги не резолвятся в previews.
  Ни заголовок, ни аннотация, ни закрытая цитата не запрашиваются из B/C/D.
  Текст, cookies и курсоры не записываются в логи или браузерное хранилище.
- Новые технические ограничения TOR: 2000 Unicode символов, 30 новых сообщений
  и 10 новых диалогов в минуту на отправителя; replay не тратит новый лимит.
  Это не заявление об идентичных численных ограничениях Python NEwBOO.
- Получение страниц: максимум 50 сообщений/диалогов, before/after keyset без
  одновременного before+after; after=0 начинает polling с первого сообщения.
  INSERT ID выделяется под pair lock, предотвращая потерю позднего commit
  при продвижении polling-курсора. Клиент опрашивает раз в 5 секунд, только
  активную вкладку; не более одного запроса одновременно, DOM ограничен 200 карточками.

## HTTP

| Метод и URL | Данные | Результат |
| --- | --- | --- |
| GET `/mail` | `after_id?, limit?` | Личный список с unread, без копий тел |
| POST `/mail/start` | `peer_id, csrf_token` | 303 на существующий/новый диалог |
| GET `/mail/{id}` | `before_id?, after_id?, limit?` | История, форма, controls, polling |
| POST `/mail/{id}/messages` | `body, command_key, csrf_token` | Сохранение/replay, 303 в диалог |
| POST `/mail/{id}/read` | `through_id, csrf_token` | Монотонное прочтение |
| POST `/mail/{id}/block` | `blocked, csrf_token` | Только собственная блокировка peer |
| GET `/api/v1/me/messages` | `conversation_id?, after_id?, before_id?, limit?` | Без conversation — mailbox; с ним — messages+has_more+ID курсоров |
| POST `/api/v1/me/messages` | JSON `{peer_id: string, text: string}` | `{message_id: string, replayed: bool}` |

JSON IDs в ответах и write-body — десятичные строки. У JSON POST только
`Idempotency-Key` и `X-CSRF-Token` в headers; client actor/key в JSON запрещён.
У HTML форм скрытые `command_key` и `csrf_token`. Во всех mutations — session,
Origin/CSRF, участник или точный peer; private responses — no-store.
Вспомогательный GET `/assets/mail.js` публичен и не содержит пользовательских данных.
Stable RIL IDs main 0–83 сохранены; новые 84–92, product area Mail.

## Передача I и приёмка

Новый пакет `tor-social` зарегистрирован существующим owner E. Передача I в этом
PR включает workspace/Cargo, девять маршрутных пар/RIL guards и новую аддитивную
миграцию `202610300040_e08_private_mail.sql`. Старые миграции не переписываются.
Номер 040 не пересекается с main; перед объединением с другими Draft требуется
повторная проверка общего графа. Проверка installed SQLx history и выкладка VPS
сюда не входят. Не переименовывать уже применённую миграцию.

Проверки на итоговом SHA:

```bash
cargo test -p tor-social --lib
cargo test -p tor-social --test slices_pg e_08_ -- --ignored --test-threads=1
cargo test -p tor-web --test mail_http_pg e_08_ -- --ignored --test-threads=1
python3 -m unittest discover -s tests/architecture -v
```

PG suite доказывает concurrent pair/replay, foreign actor, monotone cursor,
block/send serialization, independent unblock, 55-message pagination,
distributed rate limit и чтение через новое соединение.
HTTP suite использует реальные account sessions/HTML command_key, session/CSRF,
JSON/body tampering, hidden previews, third user, read-state и stale-form block.
`mail_browser_pg.cjs` запускается существующим reader workflow на PG18:
390/1440, три аккаунта, отправка/replay/polling/read, block/unblock,
plain-text XSS safety, проверка ширины и отправка без JavaScript.

За пределами этого среза: групповые комнаты, admin mailbox, адресная книга/
дружба/настройки входящих, вложения и encrypted messaging, письма на email,
push/email уведомления G, rich book previews, удаление/редактирование сообщений.
Полный S057 не объявлять принятым по одной компиляции или наличию этого документа.
