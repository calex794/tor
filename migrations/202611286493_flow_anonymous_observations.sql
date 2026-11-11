-- Independent short-lived per-card proofs. No reader/account/session/IP/URL fields.
CREATE TABLE flow_observation_state (
 singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton),
 live_proofs bigint NOT NULL DEFAULT 0 CHECK(live_proofs BETWEEN 0 AND 100000)
);
INSERT INTO flow_observation_state(singleton) VALUES(true);
CREATE TABLE flow_observation_proofs (
 digest bytea PRIMARY KEY CHECK(octet_length(digest)=32),
 book_id bigint NOT NULL CHECK(book_id>0),
 surface text NOT NULL CHECK(surface IN ('home','catalog','search','book_detail','reader_end')),
 campaign_id bigint NOT NULL DEFAULT 0 CHECK(campaign_id>=0),
 campaign_revision bigint NOT NULL DEFAULT 0 CHECK(campaign_revision>=0),
 slot text NOT NULL DEFAULT '' CHECK(slot IN ('','home_featured','book_detail_related','chapter_end_free')),
 issued_at timestamptz NOT NULL DEFAULT clock_timestamp(),
 expires_at timestamptz NOT NULL DEFAULT (clock_timestamp()+interval '15 minutes'),
 bucket timestamptz NOT NULL DEFAULT (date_trunc('hour',now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'),
 rendered boolean NOT NULL DEFAULT false, viewed boolean NOT NULL DEFAULT false, clicked boolean NOT NULL DEFAULT false,
 CHECK(expires_at>issued_at AND expires_at<=issued_at+interval '901 seconds'),
 CHECK((campaign_id=0 AND campaign_revision=0 AND slot='') OR (campaign_id>0 AND slot<>'')),
 CHECK(NOT viewed OR rendered), CHECK(NOT clicked OR rendered)
);
CREATE INDEX flow_observation_proof_expiry ON flow_observation_proofs(expires_at,digest);
CREATE TABLE flow_observation_hourly (
 bucket timestamptz NOT NULL,
 book_id bigint NOT NULL CHECK(book_id>0),
 surface text NOT NULL CHECK(surface IN ('home','catalog','search','book_detail','reader_end')),
 campaign_id bigint NOT NULL DEFAULT 0 CHECK(campaign_id>=0),
 slot text NOT NULL DEFAULT '' CHECK(slot IN ('','home_featured','book_detail_related','chapter_end_free')),
 opportunities bigint NOT NULL DEFAULT 0 CHECK(opportunities>=0),
 rendered bigint NOT NULL DEFAULT 0 CHECK(rendered BETWEEN 0 AND opportunities),
 views bigint NOT NULL DEFAULT 0 CHECK(views BETWEEN 0 AND rendered),
 clicks bigint NOT NULL DEFAULT 0 CHECK(clicks BETWEEN 0 AND rendered),
 expired_unobserved bigint NOT NULL DEFAULT 0 CHECK(expired_unobserved BETWEEN 0 AND opportunities),
 replayed bigint NOT NULL DEFAULT 0 CHECK(replayed>=0),
 PRIMARY KEY(bucket,book_id,surface,campaign_id,slot),
 CHECK((campaign_id=0 AND slot='') OR (campaign_id>0 AND slot<>''))
);
CREATE INDEX flow_observation_book_report ON flow_observation_hourly(book_id,bucket);
CREATE INDEX flow_observation_campaign_report ON flow_observation_hourly(campaign_id,bucket) WHERE campaign_id>0;

-- Issued metadata cannot be retargeted; accepted bits only move forward.
CREATE FUNCTION flow_observation_proof_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF (NEW.digest,NEW.book_id,NEW.surface,NEW.campaign_id,NEW.campaign_revision,NEW.slot,
     NEW.issued_at,NEW.expires_at,NEW.bucket) IS DISTINCT FROM
    (OLD.digest,OLD.book_id,OLD.surface,OLD.campaign_id,OLD.campaign_revision,OLD.slot,
     OLD.issued_at,OLD.expires_at,OLD.bucket)
    OR (OLD.rendered AND NOT NEW.rendered) OR (OLD.viewed AND NOT NEW.viewed)
    OR (OLD.clicked AND NOT NEW.clicked) THEN
   RAISE EXCEPTION 'immutable observation proof' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER flow_observation_proof_immutable BEFORE UPDATE ON flow_observation_proofs
 FOR EACH ROW EXECUTE FUNCTION flow_observation_proof_guard();
