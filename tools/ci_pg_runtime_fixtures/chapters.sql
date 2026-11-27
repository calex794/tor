INSERT INTO chapters (id, book_id, chapter_no, title, visibility, access_level, price_amount, reader_released_at)
VALUES
  (101, 1, 1, 'Начало истории', 'public', 'free', 0, NULL),
  (102, 1, 2, 'Секретный черновик', 'draft', 'free', 0, NULL),
  (103, 1, 3, 'Платная глава', 'public', 'paid', 250, NULL),
  (104, 2, 1, 'Глава скрытой книги', 'public', 'free', 0, NULL),
  (105, 3, 1, 'Глава заблокированной книги', 'public', 'free', 0, NULL),
  (106, 1, 4, 'Запланированная глава', 'public', 'free', 0, now() + INTERVAL '1 day'),
  (107, 1, 5, 'Глава на модерации', 'public', 'free', 0, NULL),
  (108, 1, 6, 'Продолжение', 'public', 'free', 0, NULL);
UPDATE chapters SET admin_hold=true WHERE id=107;
INSERT INTO chapter_versions (id, chapter_id, version_no, body_markdown)
VALUES
  (201, 101, 1, '**Первая строка** и <script>alert(1)</script>'),
  (202, 102, 1, 'PRIVATE-DRAFT-MARKER'),
  (203, 103, 1, 'PRIVATE-PAID-MARKER'),
  (204, 104, 1, 'PRIVATE-BOOK-MARKER'),
  (205, 105, 1, 'PRIVATE-HOLD-MARKER'),
  (206, 106, 1, 'PRIVATE-FUTURE-MARKER'),
  (207, 107, 1, 'PRIVATE-MODERATED-MARKER'),
  (208, 108, 1, 'Текст следующей опубликованной главы');
INSERT INTO chapter_publications(chapter_id, version_id)
VALUES (101,201),(102,202),(103,203),(104,204),
       (105,205),(106,206),(107,207),(108,208);
