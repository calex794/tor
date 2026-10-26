# I-02: общие идентификаторы и ключи команд

Первый блок I-02 реализован в `crates/contracts`, владелец — линия I. Он выполняет правила [принятых решений, §3](../roadmap/decisions-v1.md) и [API v1](api-evolution-v1.md). Зависимости только Serde; Axum, SQLx и зависимости на предметные пакеты отсутствуют.

`UserId`, `BookId`, `ChapterId`, `ChapterVersionId`, `PurchaseId` — разные Rust-типы с закрытым положительным `i64`. Конструкторы `TryFrom<i64>` и `TryFrom<u64>` отвергают ноль, отрицательные значения и выход за PostgreSQL BIGINT. `get()` возвращает значение для существующих адаптеров БД. Идентификатор покупки сам по себе не доказывает право доступа.

Serde читает и пишет ID только строками, включая `i64::MAX` и значения выше `Number.MAX_SAFE_INTEGER`. Числовые JSON-токены, знаки, пробелы, дроби, Unicode-цифры и переполнение запрещены. Для совместимости с существующим C-09 допустимы ведущие нули в строке длиной до 19 байт; на выходе — обычная десятичная строка без ведущих нулей. `PositiveId` сохраняет общий инвариант на старой нетипизированной границе `reading::services::sync::positive_id`; он не заменяет предметные ID в новых DTO.

`IdempotencyKey` — проверенная строка из 43 ASCII-символов `A-Z a-z 0-9 - _`, без padding. Генератор должен использовать 32 случайных байта. Валидация сохраняет историческую грамматику таблиц команд и не оценивает случайность или дополнительные ограничения последнего символа. Конструкторы `new_book` и `save_chapter` преобразуют входную строку в этот тип; SQL-адаптеры сохраняют его через `as_str()`. Область Actor/route/command и сравнение payload по-прежнему обеспечиваются владельцем транзакции.

Внешние формы и ответы не изменены. Старый `GET /api/v1/books` сохраняет числовой ID и JSON-массив. Эта поставка не добавляет новый writer, сессии, разрешения или миграции и не объявляет весь I-02 завершённым: Actor/Permission, ошибки, междоменные порты и события остаются отдельными последующими блоками той же задачи.

Проверки контракта:

```sh
cargo test -p tor-contracts
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
python3 -m unittest discover -s tests/architecture -v
python3 tools/check_feature_layout.py
python3 tools/check_contracts.py
python3 tools/check_roadmap.py
```

Регрессия использует существующие `tor-books --test authoring_pg`, `tor-reading --test slices_pg` и `tor-web --test auth_postgres` с `--ignored --test-threads=1`, только на отдельной `tor_ci` с реальной PostgreSQL и CI-фикстурами. Для браузера — `tools/author_chapter_visual_smoke.cjs`: регистрация, создание черновика, редактирование версии, явная публикация и чтение при ширине 390/1440.
