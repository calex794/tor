SELECT setval(pg_get_serial_sequence('users','id'), GREATEST((SELECT max(id) FROM users),1), true);
SELECT setval(pg_get_serial_sequence('books','id'), GREATEST((SELECT max(id) FROM books),1), true);
SELECT setval(pg_get_serial_sequence('chapters','id'), GREATEST((SELECT max(id) FROM chapters),1), true);
SELECT setval(pg_get_serial_sequence('chapter_versions','id'), GREATEST((SELECT max(id) FROM chapter_versions),1), true);
