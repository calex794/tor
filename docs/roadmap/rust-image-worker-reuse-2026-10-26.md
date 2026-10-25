# TOR: повторное использование Rust image-worker NEwBOO (26.10.2026)

**S025 / G-05, G-07, G-09, I-07.** Не писать обработку обложек заново, не создавать второй сайт, монорепо или постоянно работающий микросервис только ради картинок.

Источник зафиксирован: [image-worker/ NEwBOO @17125ef](https://github.com/HeisLuka/newboo/tree/17125ef758a791e9630505c14d24f7fe65aed228/image-worker). Старый [Python upload/callback](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/services/book_cover_service.py); историческая [карточка Notion](https://app.notion.com/p/40432a84beec8186bd08d8e7461124c8). Фактическая проверка: статически изучены Cargo manifest, основные Rust файлы, Python, Docker, часть тестов и новая архитектура TOR. `cargo test` и работающий S3 здесь НЕ запускались. Обработчик всё ещё НЕ перенесён в TOR.

## Что переносить как готовую Rust-основу

| Исходник | Функция | Новая точка G |
| --- | --- | --- |
| `src/image_pipeline/validate.rs` | JPEG,PNG,WebP magic; size | `crates/ingest/src/services/image_pipeline/validate.rs` |
| `src/image_pipeline/decode.rs` | `image` decoder, max_width/height/pixels | `.../decode.rs` |
| `src/image_pipeline/transform.rs` | crop-to-fill, Lanczos3 (`fast_image_resize`), cascade, Rayon variants | `.../transform.rs` |
| `src/image_pipeline/encode_webp.rs` | WebP encoder | `.../encode_webp.rs` |
| `src/image_pipeline/orchestrator.rs` | validate→decode→resize→output | `.../orchestrator.rs` |
| `src/image_pipeline/types.rs` | OutputFile metadata/bytes | G domain DTO |
| `src/storage/s3_client.rs` | AWS SDK S3-compatible (Yandex Object Storage) | объединить с уже утверждённым G `crates/storage`, не делать второй S3-client |

Два target из `config.rs`: **main 380×570**, **card 150×220**. Форматы исходника: JPEG/PNG/WebP. Используются Rust/Tokio/Axum, AWS S3 SDK, `image`, `fast_image_resize`, `rayon`, `moka`; из них `moka` здесь НЕ использовать как durable authority.

Это старый standalone **binary** `main.rs`, без `lib.rs`. Переносить исходные функции как модули существующего G Cargo-пакета с нормальными unit тестами. `routes.rs`, `callback.rs`, `key_parser.rs` и `main.rs` целиком не встраивать: они связаны с Python и прежней S3 key schema.

## Обнаруженные проблемы. Не переносить без исправления.

| ID | Проверенный факт и эффект | Обязательная коррекция |
| --- | --- | --- |
| IMG-P0-01 | `routes.rs` открывает `POST /s3-event` и `POST /debug/process` без проверки токена; `main.rs` всегда связывает слушатель с `0.0.0.0` несмотря на `APP_HOST`. Внешний JSON определяет bucket/key. | Не монтировать debug route в TOR; все job запускаются по авторизованным B/G командам и доступным source objects; private network+auth при редком самостоятельном worker. |
| IMG-P0-02 | `image-worker/.env.example` содержит строки, **похожие** на настоящие YC credentials; `config.rs` допускает сомнительный стандартный callback token. | Не копировать значения, проверить действительность, при необходимости ротировать. Secret-store/env только для исполнителя, нет секретов в CI/PR/logs. Наличие строки не доказывает, что ключ действителен. |
| IMG-P0-03 | `s3_event_handler` возвращает HTTP 200 после ошибок обработки отдельного объекта → источник может не повторить событие. | G PG durable job: ACK после постановки в очередь, CAS lease/fence/retry/quarantine; бизнес-эффект B только после подтверждённого S3 и PG. |
| IMG-P0-04 | `moka` dedup не сохраняется после restart, `contains_key→insert` неатомарен, при ошибке image pipeline не вызывается `unmark`. `IdempotencyCache::new(24*60*60,100000)` передаёт лимит и TTL в обратном смысле. | PG UNIQUE(source_object_id,content_hash,recipe_version), persisted job receipt, two-worker race; retry при error и после crash, zero duplicate attached cover. |
| IMG-P1-05 | В `encode_webp(img,_quality)` параметр качества **игнорируется**: фактически используется `WebPEncoder::new_lossless`. Конфиг 80/85 не делает сжатие lossy 80/85. | Или явно закрепить lossless, или реализовать lossy quality с golden output/размером/визуальным сравнением. Не выдавать lossless за настроенное качество. |
| IMG-P1-06 | `download_object_limited` сначала `body.collect()`, потом проверяет фактический размер, если Content-Length отсутствует/ложный. Старый max 25MiB/25MP, новый TOR 8MiB/24MP. | Stream bounded по фактически полученным байтам, отклонять до decode, ограничить memory/concurrency; на 1GB VPS старт с 1 CPU task. |
| IMG-P1-07 | `output_path.rs` всегда пишет `public/books/.../*.webp` и игнорирует `S3_TARGET_PREFIX`; `key_parser.rs` не принимает book ID короче 3 символов, включая 1. | Новые immutable object_uuid keys и B owned attachment/slot; support positive numeric IDs, no public paid chapter asset. |
| IMG-P1-08 | `try_join_all` может успеть загрузить один variant, а второй сломаться; без transactional attach остаётся orphan/неполное поколение. | Staging all variants, HEAD/hash verify, один B PG switch, retire прежнее поколение, G bounded GC. |
| IMG-P1-09 | В `image-worker/tests/pipeline_tests.rs` только два негативных теста; положительный decode/resize/encode на реальном файле не доказан. | JPEG/PNG/WebP golden 380×570/150×220, alpha/crop, corrupt/polyglot/metadata/animation/EXIF, old Python security parity. |

Старый Python `image_upload_security_service.py` содержит отдельные проверки анимации, метаданных и trailing bytes; нельзя понизить защиту при переносе. Отличить cover public variant от private encrypted chapter image по [контракту S3](../contracts/import-storage-v1.md).

## Целевая последовательность в TOR (без отдельного worker HTTP)

1. H/A/B: владелец книги запрашивает upload, проверяются Actor, slot, expected_revision.
2. G через `/api/v1/uploads/prepare` выдаёт scoped signed PUT только в **PRIVATE staging**; `finalize` проверяет byte limit, SHA/magic/dimensions.
3. G PG `image_variant_jobs` с UNIQUE(object,recipe_version), lease/fence и idempotent retry, source verified.
4. `spawn_blocking` + CPU semaphore (по умолчанию **1** на 1-vCPU VPS) запускает адаптированный старый Rust pipeline; не забивать Tokio reactor и не размножать Rayon tasks без измерений.
5. Два выхода staging WebP `main 380x570` и `card 150x220` проверяются по SHA/HEAD; только успешный B PG commit заменяет current cover slot и посылает outbox `CoverVariantsReady`. Старую генерацию G удаляет позже после reference+grace recheck.
6. H показывает состояний `pending/ready/failed`, затем обе производные картинки по актуальному B public/private статусу, без прежних stale aliases.

**Тесты в рамках существующих G-05/G-07/G-09:** IMG-01 pure golden decode/crop/WebP; IMG-02 bound 8MiB/24MP±1/EXIF/trailing; IMG-03 path+source allowed vs wrong book/user; IMG-04 two workers/retry/pipeline fail/crash; IMG-05 one PUT success+one failure+retry; IMG-06 source original/old cover retained until new generation committed; IMG-07 new public object no private paid leak; IMG-08 PG+real S3+Chromium and a fixed final SHA. Не добавлять к 117 строкам новые задачи и не включать замороженный URL crawler F021/S024.

**CI impact:** `crates/ingest/src/services/image_pipeline/**` запускает дешёвые G image tests; изменение G storage/attachment/ACL требует B+C+G+I PG/security+browser; полный прогон релиза отдельно. Не поднимать все A–I тесты при одной смене WebP quality без изменения контрактов.

**Статус:** исходники пригодны для переноса, но надо проверить компиляцию в новом Cargo, положительные golden-тесты, ресурсы, S3, PG и права. Не заявлять S025 реализованным по наличию этого документа.
