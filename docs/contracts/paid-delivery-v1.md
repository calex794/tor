# TOR — paid S3 direct delivery, contract v1 (план реализации)

**Состояние:** обязательный проектный контракт после принятия документационного PR; НЕ доказательство реализованного API/S3/PostgreSQL. Источник бизнес-правил: [HeisLuka/newboo @17125ef758](https://github.com/HeisLuka/newboo/tree/17125ef758a791e9630505c14d24f7fe65aed228). Новая реализация: `calex794/tor`, Rust/Axum + PostgreSQL. [Tela: 35 подшагов и 20 отказных тестов](https://telawiki.com/spaces/430/pages/7736/tor-s3). Единые [инструкции B/C/D/G/H/I](https://telawiki.com/spaces/430/pages/7725/bookarust-main-100-newboo).

**Первый релиз:** права/покупки проверяет домашний или staging Rust API; браузер получает **ciphertext напрямую из приватного S3**. Нет paid plaintext в публичном S3, HTML, общем кеше или GET-ответе API. Текст платной главы **не** передаётся Rust-сервером каждому читателю. Авторизация и выдача grant остаются через backend; S3 storage/GET/egress оплачиваются облаку. **Никакой DRM не может предотвратить копирование уже просмотренного текста**; подписанные ссылки — временные bearer credentials.

## 1. Владельцы и границы Cargo

- **I** владеет `crates/contracts`, root Cargo, маршрутизацией в `crates/web`, порядком миграций/CI, этим документом и изменениями общих API; добавляет `storage = "G"` в `architecture/modules.toml`.
- **G** владеет новым `crates/storage`: объектное хранилище/crypto/adapters/registry/GC/presign/CORS-проверки и процесс прямой загрузки медиа. Не помещать S3 в глобальный service-модуль.
- **B** (`crates/books`) владеет книгой, публикацией и immutable `ChapterVersion`, media associations. Бизнес-версия всегда у B, физический S3-объект у G.
- **D** (`crates/commerce`) единственный источник решений `EntitlementDecision`, версионных прав, фактов покупки, денежных проводок и отзыва.
- **C** (`crates/reading`) — HTTP grant, orchestration typed ports D+B+G, без SQL или S3 secret в API. H (`crates/web/assets` / templates) — WebCrypto/Markdown/PUA frontend.
- В feature Cargo нет прямых B→D, C→G и т.д.; только узкие типы/порты в `crates/contracts` и wiring в I/web. Сервисы не импортируют SQLx; SQL в repositories.

## 2. Точные HTTP-контракты

### GET /api/v1/chapters/{chapter_id}/read?purchase_id=...

**Ломающее изменение относительно первой редакции api-v1:** этот GET отдаёт **только reader metadata** и ссылку на разрешённый путь получения объекта; поле `body_markdown` не появляется в paid JSON и не появляется в бесплатном JSON при прямом public CDN. `purchase_id` (если есть) служит контекстом выбора уже авторизованной покупки, но никогда не заменяет PG проверку user ownership+revoked.

- `access=free` (published и публичная): `{chapter_id:"42",version_id:"106",access:"free",content_access:{mode:"public_s3",url:"https://.../public/chapters/42/hash.md"}}`, URL только к public versioned plaintext; курсор/читалка сохраняют контракт версии.
- `access=purchased` (или member/owner по policy): `{chapter_id:"42",version_id:"106",access:"purchased",content_access:{mode:"grant_required",grant_endpoint:"/api/v1/reader/chapters/42/delivery-grants"}}`; **без** plaintext, signed URL, DEK.
- `locked/refunded`: HTTP 403 либо согласованный metadata-paywall без текста; `hidden/draft/not found` → 404 без утечек.
- Все ответы reader/paid metadata: `Cache-Control: private, no-store`, `X-Request-Id`.

### POST /api/v1/reader/chapters/{chapter_id}/delivery-grants

- Только аутентифицированная cookie-сессия/Actor A; `Origin` + CSRF по общему договору (POST не изменяет баланс; денежный `Idempotency-Key` сюда не нужен). Тело: `{}` или `{"purchase_id":"123"}`; **не принимать** `version_id`, `object_key`, bucket, произвольный URL, raw id/version из браузера как право.
- C вызывает D.resolve_entitlement(Actor,chapter_id,purchase_id?) → точная Allowed(pinned_version_id)/Denied. Через B получает Published/entitled immutable version и ObjectRef; G проверяет private object state и подписывает URL. Если D или G недоступен — 503 без ключа.
- Response `200` (всегда no-store + noindex, never shared cache), JSON schema `tor.paid_delivery.v1`:

```json
{
  "schema":"tor.paid_delivery.v1",
  "chapter_id":"42",
  "version_id":"106",
  "expires_at":"2026-10-26T12:00:00Z",
  "content":{
    "ciphertext_url":"https://storage.example.test/private/chapter-versions/uuid.enc?signature=...",
    "object_sha256":"<64 lowercase hex>",
    "cipher":"AES-256-GCM",
    "codec":"gzip",
    "nonce_b64u":"<base64url>",
    "aad_b64u":"<base64url>",
    "dek_b64u":"<base64url of the 32-byte **scoped** version key>"
  },
  "protected_mode":"pua_v1",
  "assets":[]
}
```

- Signed GET TTL = **90 секунд** в v1; верхняя граница настроек **120 секунд**. Ссылка действует повторно до истечения TTL и может быть переслана. Refund/revocation прекращают **новые** grants, но уже выданную ссылку/скачанный plaintext отозвать нельзя.
- Ответ отказа стандартизирован: 401 unauthenticated; 403 known unauthorized/refunded; 404 hidden/nonexistent; 409 pinned race/conflict; 429 rate limit; 503 S3/PG unavailable. Единый error envelope из api-v1, без S3 URL/ключа/body даже в diagnostic metadata. Rate limit по actor/session/IP без неоправданного блокирования законных повторов.
- Не логировать signed query, DEK, KEK, plaintext. `Referrer-Policy: no-referrer`, `Cache-Control: private, no-store`, CSP ограничивает `connect-src` S3 endpoint. S3 bucket policy не даёт анонимного GET.

### POST /api/v1/reader/chapters/{chapter_id}/assets/{object_id}/delivery-grants

**Для платных иллюстраций**: такой же Actor→D entitlement→B membership (asset обязан принадлежать точной разрешённой версии)→G signed GET+scoped media DEK. Нельзя доверять `object_id` вне связки B.chapter_version_assets. DTO как chapter grant, но `codec:"none"` для уже сжатой PNG/WebP, MIME после decrypt валидируется. Не открывать старый NEwBOO публичный `chapter-images` путь для paid images.

### Авторские загрузки

Небольшой Markdown (≤50k Unicode chars по контракту B) приходит через author API **один раз** и в процессе публикации сохраняется зашифрованным в S3; это не reader egress. Большие файлы: `POST /api/v1/uploads/prepare` → авторизованный short-lived signed PUT на staging namespace → `POST /api/v1/uploads/finalize` после HEAD/size/hash/content-signature/MIME/pixels/scanning и PG attachment. Невыполненное finalize не делает объект публичным. Конкретный upload body-size из `data-and-events.md`; не смешивать с import payload.

## 3. Формат ciphertext — `tor.encrypted.chapter.v1`

- Каждая immutable версия (и каждый платный image-объект) имеет **свой независимый CSPRNG DEK** ровно 32 байта. Для этой версии произвести 12-byte random nonce; комбинация DEK+nonce уникальна. Cipher **AES-256-GCM** с tag ровно 16 байт; тело S3 — `ciphertext || tag`, Content-Type `application/octet-stream`, **без** Content-Encoding:gzip.
- Исходный Markdown — UTF-8, с договорённой B нормализацией, затем GZIP (RFC1952), затем AES-GCM. Для PNG/WebP/JPEG codec `none`. `plaintext_sha256` от исходного нормализованного Markdown привязан к версии.
- **Строгий AAD encoding** для paid markdown: UTF-8 байты соединения `"tor.encrypted.chapter.v1\n" + decimal(chapter_id) + "\n" + decimal(version_id) + "\n" + lowercase_UUID_object_id + "\n" + lowercase_sha256_hex_plaintext + "\n"`. UUID с дефисами в canonical lower-hex; десятичные ID без нулей слева. Для media заменить namespace на `tor.encrypted.asset.v1\n` и добавить точный `chapter_version_id` вместо произвольного пути. Браузер использует **полученные через разрешённый grant AAD байты**, не собственную догадку об их сборке.
- `sha256_ciphertext` (ciphertext+tag) и `size_bytes` для S3 регистрируются G до attach. Browser проверяет hash перед decrypt, AES-GCM auth tag при decrypt. Любое несовпадение = ошибка, fallback к plaintext запрещён.
- KEK отдельный от JWT/cookie `SECRET_KEY`, 32-byte secret в секретном окружении `TOR_CONTENT_KEK_V1`. Для wrapped DEK AES-256-GCM с **другим** 12-byte nonce и AAD `"tor.wrap.v1\n" + lowercase_UUID_object_id + "\n"`; в PostgreSQL `wrapped_dek, wrap_nonce, kek_id`, **никогда** raw DEK. Ротация KEK — rewrap, не изменение купленных объектов. Для восстановления необходимы PG snapshot + ciphertext S3 + независимый бэкап KEK.
- Immutable object keys: `private/chapter-versions/{uuid}.enc`, `private/chapter-assets/{uuid}.enc`. Никаких внешних непроверенных `key` из браузера. G должен защитить от overwrite через версионирование/условный PUT с проверенным поведением конкретного S3. Каждый object привязан к version через PG FK/registry.
- **Rust↔JS golden fixtures до параллельного кода**: plaintext с кириллицей/emoji/CJK, фиксированный ключ, nonce, AAD, expected ciphertext/tag/sha256/gzip bytes и expected decode. Варианты tampered byte, bad tag, wrong AAD, wrong DEK, wrong KEK (fail closed). Один кодек, никакого незапланированного browser polyfill.

## 4. PostgreSQL таблицы, публикация, отзыв

**I выполняет общую миграционную последовательность**, но таблицы имеют владельца:
- G: `storage_objects(id UUID PK, object_kind, bucket, object_key UNIQUE, state pending|ready|attached|deleting, size_bytes, sha256_ciphertext, encrypted_dek, wrap_nonce, kek_id, object_nonce, object_aad, created_at, delete_after)`; registry/GC, без plaintext DEK.
- B: `chapter_versions` добавляет `storage_object_id UUID FK` и `public_storage_object_id UUID NULL FK` (не меняет immutable body); B владеет `chapter_version_assets(chapter_version_id, storage_object_id, kind, ordinal)`. Обновление редакции создаёт V2, не UPDATE V1. Public URL только когда `published && free && !delayed`.
- D: сохраняет существующие таблицы `chapter_purchase_facts`, `library_purchased_chapters`, `chapter_purchase_revocations`, `reader_wallet_accounts`, `reader_wallet_entries` и S0 checkout atomicity; grant проверяет **committed active purchase** пользователя и закреплённую V1. Удобочитаемая библиотека/прогресс C **не** создаёт entitlement.
- I/G: `outbox_events` и fenced cleanup jobs после commit. Никакой distributed transaction PG+S3: reserve `pending` version/object → G upload → G verify HEAD/hash → PG transaction publish + attach + outbox. Fail upload = unpublished; fail PG after upload = orphan, eventual GC. **Никогда** не удалять paid pinned V1 при появлении V2.

**free→paid:** убрать public entitlement, удалить public object и purge CDN; старые скопированные файлы вернуть невозможно. **paid→free:** только после принятой публикации создавать доступный public object. Проверять reverse, deferred publication и гонки двух публикаций.

## 5. Browser и сохранение защиты NEwBOO

- `fetch` signed GET **не на API backend**, а напрямую к S3 private origin с точным allow-host; WebCrypto `crypto.subtle.decrypt({name:"AES-GCM",iv,additionalData,tagLength:128})`, затем gunzip, UTF-8 decode, **санитизация Markdown** и рендер без raw `innerHTML`. HTTPS Secure Context, закрытые CSP/Referrer-Policy. S3 CORS allow только реальный HTTPS site origin и методы GET/HEAD.
- DEK/signed URL держать только в памяти текущей читательской вкладки; не сохранять в localStorage/IndexedDB/SW/preload, не записывать в аналитику. При истечении 90s URL/обрыве повторить POST grant (с новой проверкой D); не использовать paid-url fallback.
- Старый NEwBOO использовал protected reader с PUA/custom fonts/JWT (флаг защиты по умолчанию true; watermark по умолчанию false). **Простая расшифровка raw Markdown в JS — НЕ доказанный паритет защиты старого ридера.** Для первой полноценной приёмки I/H/C/G фиксируют `pua_v1` совместимый формат и авторизованную выдачу font profiles либо отмечают явное продуктовое отступление; нельзя копировать лицензии/шрифты без проверки происхождения. Легальный клиент технически всё равно может сохранить текст.

## 6. Исполнительские шаги, проверки и критерий «готово»

**Выполнять в существующих B/C/D/G/H/I (не создавая 10-ю линию или новую папку статусов).** Конкретные 35 substeps S3-01…S3-35 и 20 adversarial T01…T20 распределены по ссылке [Tela](https://telawiki.com/spaces/430/pages/7736/tor-s3). Их обязательные подмножества находятся в исполнительских страницах каждой линии; верхнеуровневых задач TOR по-прежнему 117.

**Блокирующий full-path на real staging:** зарегистрирован U1/U2, опубликована paid V1, U1 совершает тестовую **атомарную** покупку из wallet PG, C выдаёт grant **только U1**, браузер скачивает signed ciphertext **не через домашний Rust**, WebCrypto показывает V1 и сохраняет позицию; автор издаёт V2, у U1 остаётся V1; refund прекращает новые grants; U2/guest/private image не читают; 90-second TTL, XSS, broken network, S3 outage, PG rollback/restore, tamper/negative AAD проверены. Финальные браузерные трассы/CI на точном принятом `tor/main SHA`, результаты в уже существующей Notion-матрице.

**CI:** изменение crypto/grant/entitlement/shared contract/PG migration → B+C+D+G+H+I + real PG/HTTP/S3+browser; leaf renderer → H+C+browser; storage GC → G+B retention+backup. Соблюдать `tools/check_feature_layout.py` и `tools/check_roadmap.py`; проверка 117 задач/69 сценариев не должна ломаться. Существующий [тестовый регламент](../roadmap/tests-and-handoffs.md). **Не допускать P0**: публичный plaintext paid/draft, grant без права, пропажа pinned V1, невозможность recovery, секреты в логах, обход private S3 origin.

## 7. Эксплуатация, затраты и развёртывание

S3 тарифицирует хранение ciphertext/public copies/backups, операции GET/PUT и исходящие байты S3→браузер; **API/home uplink** расходуется на запросы, HTML и однократную авторскую загрузку, но не paid body. Мониторинг grant success/denied, S3 egress/GET, рост PG/WAL/SSD, orphan GC, ошибки, бюджетные уведомления 50/80/100% выбранного лимита (alert ≠ hard cap). Не логировать подписанные ссылки и DEK.

По утверждённой линии I staging пока идёт на отдельном VPS, а потенциальный домашний i3-10105 — другая хостинг-цель. HTTP/S3 контракт не зависит от места размещения backend. Не менять топологию без решения владельца, не включать боевые деньги/секреты до тестовой PG и backup/restore.