-- FLOW-09: two aggregate rules and a fixed four-slot live availability sample.
-- This is not a URL inventory, audience collector or external uptime service.
ALTER TABLE flow_signal_states DROP CONSTRAINT flow_signal_states_rule_check;
ALTER TABLE flow_signal_states ADD CHECK(rule IN (
  'html_quality','html_change','reading_starts','reading_completions',
  'google_quality','google_clicks','yandex_quality','yandex_clicks',
  'exposure_quality','exposure_change','underexposed_books','underexposed_genres','public_page_loss'));
ALTER TABLE flow_signal_states DROP CONSTRAINT flow_signal_states_reason_check;
ALTER TABLE flow_signal_states ADD CHECK(reason IN (
  'within_threshold','recorded_drop','recorded_growth','missing_measurement',
  'partial_coverage','source_failure','discarded_observations','limited_pages',
  'small_baseline','source_unavailable','source_disabled','low_exposure','observer_coverage','public_page_loss'));
ALTER TABLE flow_signal_states DROP CONSTRAINT flow_signal_states_episode_reason_check;
ALTER TABLE flow_signal_states ADD CHECK(episode_reason IN (
  'recorded_drop','recorded_growth','partial_coverage','source_failure',
  'discarded_observations','limited_pages','low_exposure','observer_coverage','public_page_loss'));
ALTER TABLE flow_signal_events DROP CONSTRAINT flow_signal_events_reason_check;
ALTER TABLE flow_signal_events ADD CHECK(reason IN (
  'within_threshold','recorded_drop','recorded_growth','missing_measurement',
  'partial_coverage','source_failure','discarded_observations','limited_pages',
  'small_baseline','source_unavailable','source_disabled','low_exposure','observer_coverage','public_page_loss'));

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
