-- Aggregate-only administrative signals; no reader, session, IP or recipient.
-- One fenced round per selected window. Source changes have independent cursors.
CREATE TABLE flow_signal_rounds (
 days integer PRIMARY KEY CHECK(days IN (0,1,7,30,90)),
 epoch bigint NOT NULL DEFAULT 0 CHECK(epoch>=0),
 started_ms bigint NOT NULL DEFAULT 0 CHECK(started_ms>=0),
 lease_until_ms bigint NOT NULL DEFAULT 0 CHECK(lease_until_ms>=0),
 committed boolean NOT NULL DEFAULT false,
 payload_digest text CHECK(payload_digest ~ '^[0-9a-f]{64}$'),
 CHECK(committed=(payload_digest IS NOT NULL))
);
INSERT INTO flow_signal_rounds(days) VALUES(0),(1),(7),(30),(90);

CREATE TABLE flow_signal_states (
 rule text NOT NULL CONSTRAINT flow_signal_states_rule_check CHECK(rule IN ('html_quality','html_change','reading_starts','reading_completions','google_quality','google_clicks','yandex_quality','yandex_clicks','exposure_quality','exposure_change','underexposed_books','underexposed_genres','public_page_loss')),
 days integer NOT NULL CHECK((rule='public_page_loss' AND days=0) OR (rule<>'public_page_loss' AND days IN (1,7,30,90))),
 scope text NOT NULL CHECK(scope ~ '^[0-9a-f]{64}$'),
 status text NOT NULL CHECK(status IN ('unknown','active','healthy','disabled')),
 reason text NOT NULL CONSTRAINT flow_signal_states_reason_check CHECK(reason IN ('within_threshold','recorded_drop','recorded_growth','missing_measurement','partial_coverage','source_failure','discarded_observations','limited_pages','small_baseline','source_unavailable','source_disabled','low_exposure','observer_coverage','public_page_loss')),
 last_epoch bigint NOT NULL CHECK(last_epoch>0),
 checked_at_ms bigint NOT NULL CHECK(checked_at_ms>0),
 until_epoch bigint CHECK(until_epoch>=0),
 window_label text NOT NULL CHECK(octet_length(window_label)<=240),
 enabled boolean NOT NULL,
 available boolean NOT NULL,
 current_value bigint CHECK(current_value>=0),
 previous_value bigint CHECK(previous_value>=0),
 current_units integer NOT NULL CHECK(current_units>=0),
 previous_units integer NOT NULL CHECK(previous_units>=0),
 expected_units integer NOT NULL CHECK(expected_units>=0),
 known_discards bigint NOT NULL CHECK(known_discards>=0),
 source_failed boolean NOT NULL,
 limited_units integer NOT NULL CHECK(limited_units>=0),
 omitted_rows bigint NOT NULL CHECK(omitted_rows>=0),
 current_ack_per_mille integer CHECK(current_ack_per_mille BETWEEN 0 AND 1000),
 previous_ack_per_mille integer CHECK(previous_ack_per_mille BETWEEN 0 AND 1000),
 current_sample bigint NOT NULL CHECK(current_sample>=0),
 previous_sample bigint NOT NULL CHECK(previous_sample>=0),
 current_opportunities bigint NOT NULL CHECK(current_opportunities>=0),
 previous_opportunities bigint NOT NULL CHECK(previous_opportunities>=0),
 unknown_entities bigint NOT NULL CHECK(unknown_entities>=0),
 episode_seq bigint NOT NULL DEFAULT 0 CHECK(episode_seq>=0),
 episode_reason text CONSTRAINT flow_signal_states_episode_reason_check CHECK(episode_reason IN ('within_threshold','recorded_drop','recorded_growth','missing_measurement','partial_coverage','source_failure','discarded_observations','limited_pages','small_baseline','source_unavailable','source_disabled','low_exposure','observer_coverage','public_page_loss')),
 episode_opened_ms bigint CHECK(episode_opened_ms>=0),
 episode_emitted boolean NOT NULL DEFAULT false,
 last_notice_ms bigint CHECK(last_notice_ms>=0),
 PRIMARY KEY(rule,days,scope),
 CHECK((episode_reason IS NULL AND episode_opened_ms IS NULL AND NOT episode_emitted)
       OR (episode_reason IS NOT NULL AND episode_opened_ms IS NOT NULL AND episode_seq>0))
);
CREATE INDEX flow_signal_latest ON flow_signal_states(days,rule,checked_at_ms DESC);
CREATE TABLE flow_signal_events (
 id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
 LIKE flow_signal_states INCLUDING DEFAULTS INCLUDING CONSTRAINTS,
 kind text NOT NULL CHECK(kind IN ('opened','recovered')),
 UNIQUE(rule,days,scope,episode_seq,kind)
);
CREATE INDEX flow_signal_event_page ON flow_signal_events(days,id DESC);
CREATE INDEX flow_signal_event_retention ON flow_signal_events(checked_at_ms,id);
