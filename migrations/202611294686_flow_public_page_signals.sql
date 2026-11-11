-- FLOW-09: a fixed current public-page sample. No audience facts or URL inventory.
CREATE TABLE flow_public_page_probe (
  singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton),
  epoch bigint NOT NULL DEFAULT 0 CHECK(epoch>=0),
  origin text CHECK(length(origin) BETWEEN 8 AND 255),
  finished_origin text CHECK(length(finished_origin) BETWEEN 8 AND 255),
  lease_until timestamptz,
  started_at timestamptz,
  finished_at timestamptz
);
INSERT INTO flow_public_page_probe(singleton) VALUES(true);
CREATE TABLE flow_public_page_results (
  slot smallint PRIMARY KEY CHECK(slot BETWEEN 1 AND 4),
  book_id bigint CHECK(book_id>0),
  chapter_id bigint CHECK(chapter_id>0),
  book_revision text CHECK(length(book_revision) BETWEEN 1 AND 20),
  chapter_revision text CHECK(length(chapter_revision) BETWEEN 1 AND 20),
  publication_revision text CHECK(length(publication_revision) BETWEEN 1 AND 20),
  version_id bigint CHECK(version_id>0),
  http_status smallint CHECK(http_status BETWEEN 100 AND 599),
  baseline_known boolean NOT NULL DEFAULT false,
  last_success_at timestamptz,
  checked_at timestamptz NOT NULL,
  CHECK((slot IN(1,2) AND book_id IS NULL AND chapter_id IS NULL AND book_revision IS NULL
      AND chapter_revision IS NULL AND publication_revision IS NULL AND version_id IS NULL)
    OR (slot=3 AND chapter_id IS NULL AND chapter_revision IS NULL AND publication_revision IS NULL
      AND version_id IS NULL AND (book_id IS NULL)=(book_revision IS NULL))
    OR (slot=4 AND (book_id IS NULL)=(chapter_id IS NULL)
      AND (book_id IS NULL)=(book_revision IS NULL) AND (book_id IS NULL)=(chapter_revision IS NULL)
      AND (book_id IS NULL)=(publication_revision IS NULL) AND (book_id IS NULL)=(version_id IS NULL))),
  CHECK(NOT baseline_known OR last_success_at IS NOT NULL)
);
