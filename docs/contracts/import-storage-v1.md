# TOR File/S3 v1 — файловый импорт и прямой зашифрованный reader без crawler (PREF-05)

**Принято:** private S3-compatible storage обязателен для платной версии; **НЕ** переправлять её через домашний Axum; бесплатные публичные объекты — отдельный public namespace. Нельзя объявить production paid reader работающим без provider+HTTPS+CORS+KEK+backup. Источник: [paid-delivery-v1](paid-delivery-v1.md), [Tela S3-01…35](https://telawiki.com/spaces/430/pages/7736/tor-s3). S023 — upload файла глав, S025 — обложки, S066 — очереди. **[S024](https://app.notion.com/p/40432a84beec8137a481c358f31df050) и [F021](https://app.notion.com/p/40432a84beec81959f52df54923ff337) заморожены без срока:** нет URL-import, crawler, site parser, scanner, самостоятельных crawler jobs/API/UI/tests. Архивный G-03 не исполнять.

## 1. Режимы файлов и точные форматы

| Назначение | Принимаемый формат | Верификация |
| --- | --- | --- |
| Chapter import | `.txt`, `.md`, `.docx`, `.zip`; внутри ZIP только `.txt/.md/.docx` (как NEwBOO [chapter_import_file_reader.py](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/services/chapter_import_file_reader.py)) | filename/real magic, UTF-8 или строго ограниченная legacy CP1251 для текста; DOCX OpenXML без внешних сущностей |
| Обложка | проверенные `image/jpeg`, `image/png`, `image/webp`; metadata EXIF strip | bytes signature, decoded pixels, not claimed MIME; RGBA→RGB conversion explicit |
| Paid chapter | Markdown UTF-8 normalized → GZIP → AES-256-GCM | B/G opaque ciphertext в PRIVATE S3, SHA-256/size/AAD + Wrapped DEK, не HTML body |
| Paid chapter image | проверенное изображение (без GZIP) → AES-256-GCM | отдельный random DEK/object, выдаётся только по праву соответствующей ChapterVersion |
| Free published chapter | B metadata + публичная копия конкретной опубликованной версии | PUBLIC S3 без денежных ключей; purge/evict при free→paid, но не обещать изъятие уже скачанного |

**Фиксированные лимиты TOR** из [data-and-events.md](../roadmap/data-and-events.md): оригинал изображения≤8,388,608 байт и ≤24 млн пикселей; ZIP upload≤33,554,432 байт, суммарная распаковка≤268,435,456 байт,≤5,000 файлов, compression ratio≤50, worker RAM≤64 МиБ; глава после извлечения≤50,000 Unicode characters, form body≤1,048,576 байт, обычный JSON≤64 КиБ. Документы DOCX тоже ZIP-контейнеры: применять те же общие guards плюс limits числа XML-elements и обход запретных XML constructs. Поскольку старый Python принимал файл до 50 MiB и ZIP ratio до 100, выбранные лимиты TOR **не равны точным старым значениям**: превышение → 413/422 и видимая причина, а не silent truncate.

**ZIP security:** абсолютные пути, `../`, `C:\`/дисковые пути, symlink, nested archive, encrypted zip member, ZIP bomb, duplicate normalised paths, XML entity expansion, external relationships, malicious content-type – deny до B commands. Пакетированные главы сортировать natural order, не лексикографический `chapter_10` перед `chapter_2`; одинаковый номер главы — явный conflict в preview и не создаёт скрыто вторую копию.

## 2. Upload/finalize и набор постоянных таблиц

**Для media:** `POST /api/v1/uploads/prepare` → A/B owner permission, filename/size/type claimed + server-chosen `upload_id/object_id` + scoped signed S3 PUT to PRIVATE **staging**, TTL≤10 min, one-time nonce. `POST /api/v1/uploads/finalize` → G HEAD/GET bounded stream verify SHA256, signature/magic/pixel/decode/size + hash immutable object, persist `storage_objects.status=verified`; только затем B связывает ChapterVersion/cover в своей PG транзакции. Не доверять пользовательскому `bucket/key` или `object_id` без prepared ownership, expiration и re-validation.

**Для .zip/.docx/chapter import:** первый POST создаёт `import_job(status=pending,actor,book_id,policy_version)`; payload хранить во временном PRIVATE объекте, job CLAIM через G `lease_epoch`; parser streaming→`parsed_chapters_preview` (название/номер/byte count/validation errors), **никакой публикации**; пользователь подтверждает ревизию; B authoritative command переносит/обновляет главы atomically и создаёт версии/outbox. При сбое N-го файла нельзя оставить «опубликованы половина книги» без явного audited partial-import contract (для V1 **all-or-nothing**). Отмена job прерывает будущие B commands, но не удаляет уже подтверждённую публикацию без команды B.

**G-owned registry (миграции только I):**
- `storage_objects(object_id UUID PK,scope public|private|staging,kind,storage_key UNIQUE,state pending|uploaded|verified|attached|deleting|deleted|quarantined,content_sha256,cipher_sha256,size_bytes,encrypted_dek,wrap_nonce,kek_id,created_at,delete_after,lease_epoch)`;
- `storage_object_attachments(object_id,owner_kind,owner_id,chapter_version_id,attached_at)`; уникальная attachment identity, FK;
- `import_jobs(job_id,user_id,book_id,kind=file_upload,status,attempt,checkpoint,lease_epoch,error_code,created_at,updated_at)` и `import_job_items(job_id,ordinal,number,title,text_hash,errors)`;
- БД хранит именно immutable object **metadata**, а не всё шифрованное тело; B оставляет свою authoritative ChapterVersion, D свою paid entitlement.

**Idempotency:** одинаковая подтверждённая загрузка `upload_id+hash` → тот же object attachment; иной bytes/hash на тот же `upload_id` →409 `UPLOAD_HASH_CONFLICT`. `job resume` продолжает после checkpoint и не создаёт второй B ChapterVersion; `job cancel` не удаляет чужой attached asset.

## 3. Условия private S3 (без выбора фиктивного провайдера)

**Required secrets/config** (имена — единый договор I/G):
- `TOR_S3_ENDPOINT` (HTTPS; публично доступен браузеру для signed GET), `TOR_S3_REGION`, `TOR_S3_BUCKET_PRIVATE`, `TOR_S3_BUCKET_PUBLIC`, `TOR_S3_ACCESS_KEY_ID`, `TOR_S3_SECRET_ACCESS_KEY`, `TOR_S3_FORCE_PATH_STYLE` (если провайдеру необходимо);
- `TOR_CONTENT_KEK_V1` — независимый секрет 256 bit; `TOR_CONTENT_KEK_ACTIVE_ID=content-kek-v1`, отдельно versioned backup escrow/recovery; старые ключи доступны read-only до GC всех paid V1;
- `TOR_APP_PUBLIC_ORIGIN` — конкретный HTTPS фронтенд, строгий allow-origin CORS GET/HEAD/OPTIONS в S3; `Content-Security-Policy` allowlisted `connect-src`; `Referrer-Policy: no-referrer` на reader.

**Без этих значений** `paid_delivery_enabled=false` и readiness `paid_capability=not_ready`. Не использовать публичный filesystem URL, память процесса или временный /tmp как замену долговечному приватному S3. Для локальных unit/integration можно MinIO/совместимый fake по фиксированному протоколу; конечная приёмка требует **реального выбранного S3 endpoint** и браузерный Network, где GET ciphertext идёт **S3→browser**, а не Axum proxy.

**Границы криптографии:** отдельный случайный DEK на private object, AES-256-GCM 96-bit nonce+128-bit tag, KMS/KEK только на сервере, отдельный wrap nonce; AAD канонически связывает schema, object kind/id, chapter/version id и content hash. Эти байты должны быть fixture Rust↔WebCrypto. Браузер получает временный разрешённый DEK конкретной редакции, **не KEK**. Не писать signed URL/DEK/plaintext в traces/logger/localStorage/HTML/caches; release после decrypt не считается неуязвимым для копирования (DRM не обещать).

## 4. Orphan/GC без уничтожения оплаченных V1

1. `pending`→S3 PUT→HEAD/checksum→`verified`→B attach/publication PG commit; orphan при неудачной PG транзакции живёт до bounded grace, не блокирует обычные uploads.
2. GC G берёт lease/fence объекта; **повторно** в одной PG транзакции проверяет отсутствие attachment всех ChapterVersions, публичных/current ссылок, оплаченных V1 pins, refund/system dispositions и active grants по policy. Только после `state=deleting,lease_epoch=...` отправляет S3 DELETE.
3. Если между claim и фактическим DELETE появляется attachment, обе стороны сериализованы: B не может прикрепить `deleting`, GC сначала завершает release или отменяет; не допускать `paid active but object deleted`.
4. S3 DELETE — idempotent; при ошибке retry/backoff и quarantine после лимита попыток, не помечать `deleted` до подтверждённого provider response. Unavailable S3 не отменяет PG purchase или публикацию, но запрещает **новый grant** при невозможности fetch/unwrap/presign.
5. Восстановление требует согласованных **PG snapshot+S3 objects+KEK** на одной логической границе; проверка restore на отдельно поднятом окружении должна открыть купленную V1 после публикации V2.

## 5. Тестовый корпус для исполнителей FILE-01…18

| ID | Вход | Результат |
| --- | --- | --- |
| 01/02 | UTF-8 .txt/.md и CP1251, CRLF/разные заголовки | одинаковое число и номера глав, no duplicate heading |
| 03 | корректный DOCX с таблицей/списком, безопасными XML | детерминированный текст в preview |
| 04 | ZIP с `chapter_2.txt`, `chapter_10.txt` | порядок 2,10 |
| 05 | ZIP `../escape`, absolute path, symlink, nested zip | rejected, никакого файлового раскрытия |
| 06 | encrypted member, XML entities, DOCX external relationship | rejected |
| 07 | 32MiB±1 byte, 256MiB uncompressed±1, 5000±1 member, ratio 50±1 | точные HTTP/worker границы, 413/422 |
| 08 | 50,000+1 Unicode chars / invalid encoding | 422, нет B publication |
| 09 | JPEG/PNG/WebP 8MiB±1, 24MP±1 | 413/422, media не attach |
| 10 | MIME claim PNG, bytes ZIP / polyglot file | rejected |
| 11 | wrong actor / expired signed PUT / uploaded other object hash | 403/409, no B mutation |
| 12 | failed PG commit after uploaded object | orphan → GC, без оплаченного/публичного артефакта |
| 13 | paid V1 → V2, old V1 pinned | V1 ciphertext/DEK retained, V2 separate object |
| 14 | guest direct private GET, expired signature / altered tag | S3 deny / browser cryptographic error, no plaintext fallback |
| 15 | free→paid→free | public API changed+CDN purge, new public snapshot; private V retained |
| 16 | two concurrent workers + interrupted network + restart | one attachment/job checkpoint, no duplicate ChapterVersion |
| 17 | S3 down/KEK missing/PG down | 503 for paid grant, no key or URL, no data corruption |
| 18 | restore PG+private/public S3+KEK in isolated staging | previous paid V1 still decrypts and resume position correct |

**Файловые фикстуры генерировать детерминированным скриптом G при реализации** из записи [FILE-01…18](import-storage-v1.md) и закрепить bytes/hash/expected parsed chapter list; не обещать существование бинарных файлов до PR. В S3 e2e нужен отдельный реально настроенный provider — зависимость владельца инфраструктуры, которую docs/CI не могут придумать.

**Приоритет:** FILE-13..18 являются **P0 для оплаченного reader**, FILE-01..12 P1 для публичного импорта/медиа; crawler никогда не возвращается в scope без нового решения владельца.
