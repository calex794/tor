-- F delivery evidence, bounded generations, durable caps and materialized views.
CREATE TABLE ad_delivery_generations (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    status TEXT NOT NULL DEFAULT 'building' CHECK(status IN('building','ready','active','retired')),
    host_cursor BIGINT NOT NULL DEFAULT 0,
    host_ceiling BIGINT NOT NULL,
    campaign_ceiling BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE TABLE ad_delivery_active (
    singleton BOOLEAN PRIMARY KEY DEFAULT true CHECK(singleton),
    generation_id BIGINT NOT NULL REFERENCES ad_delivery_generations(id)
);
CREATE TABLE ad_delivery_assignments (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    generation_id BIGINT NOT NULL REFERENCES ad_delivery_generations(id),
    host_book_id BIGINT NOT NULL REFERENCES books(id),
    publisher_id BIGINT NOT NULL REFERENCES users(id),
    campaign_id BIGINT NOT NULL REFERENCES ad_chapter_campaigns(id),
    rank INTEGER NOT NULL CHECK(rank BETWEEN 1 AND 4),
    UNIQUE(generation_id,host_book_id,campaign_id),
    UNIQUE(generation_id,host_book_id,rank)
);
CREATE INDEX ad_assignments_serving ON ad_delivery_assignments(generation_id,host_book_id,rank);
CREATE TRIGGER ad_assignment_immutable BEFORE UPDATE OR DELETE ON ad_delivery_assignments
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();
CREATE TABLE ad_viewability_opportunities (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    token_hash BYTEA NOT NULL UNIQUE CHECK(octet_length(token_hash)=32),
    assignment_id BIGINT NOT NULL REFERENCES ad_delivery_assignments(id),
    viewer_id BIGINT NOT NULL REFERENCES users(id),
    session_hash BYTEA NOT NULL CHECK(octet_length(session_hash)=32),
    host_book_id BIGINT NOT NULL REFERENCES books(id),
    host_chapter_id BIGINT NOT NULL REFERENCES chapters(id),
    issued_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    expires_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()+interval '5 minutes',
    CHECK(expires_at>issued_at AND expires_at<=issued_at+interval '5 minutes')
);
CREATE INDEX ad_opportunity_expiry ON ad_viewability_opportunities(expires_at,id);
CREATE INDEX ad_opportunity_viewer_budget ON ad_viewability_opportunities(viewer_id,issued_at DESC);
CREATE TRIGGER ad_opportunity_immutable BEFORE UPDATE OR DELETE ON ad_viewability_opportunities
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();
CREATE TABLE ad_frequency_buckets (
    campaign_id BIGINT NOT NULL REFERENCES ad_chapter_campaigns(id),
    subject_hash BYTEA NOT NULL CHECK(octet_length(subject_hash)=32),
    bucket TEXT NOT NULL CHECK(bucket IN('day','hour','session')),
    window_start TIMESTAMPTZ NOT NULL,
    count INTEGER NOT NULL CHECK(count>0),
    expires_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY(campaign_id,subject_hash,bucket,window_start)
);
CREATE INDEX ad_frequency_expiry ON ad_frequency_buckets(expires_at);
CREATE TABLE ad_impression_reservations (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    opportunity_id BIGINT NOT NULL UNIQUE REFERENCES ad_viewability_opportunities(id),
    campaign_id BIGINT NOT NULL REFERENCES ad_chapter_campaigns(id),
    intent_id BIGINT NOT NULL REFERENCES finance_ad_funding(intent_id),
    ordinal BIGINT NOT NULL CHECK(ordinal>0),
    publisher_id BIGINT NOT NULL REFERENCES users(id),
    gross_minor BIGINT NOT NULL CHECK(gross_minor>=0),
    publisher_minor BIGINT NOT NULL CHECK(publisher_minor>=0 AND publisher_minor<=gross_minor),
    platform_minor BIGINT NOT NULL CHECK(platform_minor>=0 AND platform_minor+publisher_minor=gross_minor),
    reservation_transaction_id BIGINT NOT NULL UNIQUE REFERENCES finance_ad_transactions(id),
    accepted_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    settlement_transaction_id BIGINT UNIQUE REFERENCES finance_ad_transactions(id),
    settled_at TIMESTAMPTZ,
    UNIQUE(intent_id,ordinal),
    CHECK((settlement_transaction_id IS NULL)=(settled_at IS NULL))
);
CREATE INDEX ad_impressions_pending ON ad_impression_reservations(campaign_id,ordinal)
    WHERE settled_at IS NULL;
CREATE FUNCTION ad_reservation_immutable() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF (to_jsonb(OLD)-'settlement_transaction_id'-'settled_at') IS DISTINCT FROM
       (to_jsonb(NEW)-'settlement_transaction_id'-'settled_at')
       OR OLD.settled_at IS NOT NULL OR NEW.settled_at IS NULL
       OR NEW.settlement_transaction_id IS NULL THEN
        RAISE EXCEPTION 'immutable impression evidence' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END; $$;
CREATE TRIGGER ad_reservation_update BEFORE UPDATE ON ad_impression_reservations
    FOR EACH ROW EXECUTE FUNCTION ad_reservation_immutable();
CREATE TRIGGER ad_reservation_no_delete BEFORE DELETE ON ad_impression_reservations
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();
CREATE TABLE ad_settlements (
    reservation_id BIGINT PRIMARY KEY REFERENCES ad_impression_reservations(id),
    transaction_id BIGINT NOT NULL UNIQUE REFERENCES finance_ad_transactions(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE TRIGGER ad_settlement_immutable BEFORE UPDATE OR DELETE ON ad_settlements
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();
CREATE TABLE finance_ad_daily_units (
    intent_id BIGINT NOT NULL REFERENCES finance_ad_funding(intent_id),
    ordinal BIGINT NOT NULL CHECK(ordinal>0),
    reservation_transaction_id BIGINT NOT NULL UNIQUE REFERENCES finance_ad_transactions(id),
    settlement_transaction_id BIGINT NOT NULL UNIQUE REFERENCES finance_ad_transactions(id),
    PRIMARY KEY(intent_id,ordinal)
);
CREATE TRIGGER finance_ad_daily_unit_immutable BEFORE UPDATE OR DELETE ON finance_ad_daily_units
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();
CREATE TABLE ad_campaign_stats (
    campaign_id BIGINT PRIMARY KEY REFERENCES ad_chapter_campaigns(id),
    accepted BIGINT NOT NULL DEFAULT 0,
    settled BIGINT NOT NULL DEFAULT 0,
    spent_minor BIGINT NOT NULL DEFAULT 0,
    publisher_minor BIGINT NOT NULL DEFAULT 0,
    platform_minor BIGINT NOT NULL DEFAULT 0,
    clicks BIGINT NOT NULL DEFAULT 0,
    opens BIGINT NOT NULL DEFAULT 0,
    reads BIGINT NOT NULL DEFAULT 0,
    library_adds BIGINT NOT NULL DEFAULT 0,
    purchases BIGINT NOT NULL DEFAULT 0
);
CREATE TABLE ad_delivery_hourly_stats (
    campaign_id BIGINT NOT NULL REFERENCES ad_chapter_campaigns(id),
    hour_start TIMESTAMPTZ NOT NULL,
    accepted BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY(campaign_id,hour_start)
);
CREATE TABLE ad_campaign_attribution (
    opportunity_id BIGINT NOT NULL REFERENCES ad_viewability_opportunities(id),
    action TEXT NOT NULL CHECK(action IN('click','open','read','library','purchase')),
    recorded_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY(opportunity_id,action)
);
CREATE TRIGGER ad_attribution_immutable BEFORE UPDATE OR DELETE ON ad_campaign_attribution
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();
CREATE INDEX finance_ad_maturity_global ON finance_ad_publisher_credits(clear_at,id) WHERE status='pending';
CREATE INDEX finance_ad_credit_owner_page ON finance_ad_publisher_credits(publisher_id,id DESC);
CREATE INDEX ad_daily_enabled ON ad_daily_offers(id) WHERE enabled;
CREATE INDEX ad_package_enabled ON ad_packages(id) WHERE enabled;
CREATE TABLE ad_attribution_pending (
 opportunity_id BIGINT PRIMARY KEY REFERENCES ad_viewability_opportunities(id),
 next_check_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
 expires_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()+interval '30 minutes'
);
CREATE INDEX ad_attribution_due ON ad_attribution_pending(next_check_at,opportunity_id);
CREATE INDEX ad_purchase_outcome_proof ON chapter_purchase_facts(user_id,acquired_at,chapter_id);
CREATE TABLE finance_ad_cancel_intents (
 intent_id BIGINT PRIMARY KEY REFERENCES finance_ad_funding(intent_id),
 actor_id BIGINT NOT NULL REFERENCES users(id),
 command_key TEXT NOT NULL CHECK(char_length(command_key)=43),
 expected_revision BIGINT NOT NULL CHECK(expected_revision>0),
 reason TEXT NOT NULL CHECK(char_length(reason) BETWEEN 1 AND 500),
 UNIQUE(actor_id,command_key)
);
CREATE TRIGGER finance_ad_cancel_intent_immutable BEFORE UPDATE OR DELETE ON finance_ad_cancel_intents
 FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();

-- The same B/A gates as editorial discovery. It grants no chapter/body access.
CREATE FUNCTION ad_book_discoverable(viewer BIGINT, book BIGINT) RETURNS BOOLEAN
LANGUAGE SQL STABLE AS $$
 SELECT EXISTS(SELECT 1 FROM rec_materializable_books b LEFT JOIN users u ON u.id=viewer
   WHERE b.id=book AND (viewer IS NULL OR u.account_state='active')
     AND (b.access_level='basic' OR (b.access_level='adult' AND (u.adult_enabled
       OR EXISTS(SELECT 1 FROM user_level_grants g WHERE g.user_id=u.id AND g.level='pro_adult' AND g.is_active)))
       OR (b.access_level='pro_adult' AND EXISTS(SELECT 1 FROM user_level_grants g
         WHERE g.user_id=u.id AND g.level='pro_adult' AND g.is_active)))
     AND NOT EXISTS(SELECT 1 FROM rec_book_preferences p WHERE p.user_id=viewer
       AND p.book_id=b.id AND p.hidden AND p.expires_at>now()))
$$;
CREATE VIEW ad_live_assignments AS
 SELECT a.*,c.advertiser_id,c.promoted_book_id,c.funding_intent_id
 FROM ad_delivery_assignments a JOIN ad_delivery_active active ON active.generation_id=a.generation_id
 JOIN ad_delivery_generations gen ON gen.id=active.generation_id AND gen.status='active'
 JOIN ad_chapter_campaigns c ON c.id=a.campaign_id AND c.status='funded'
 JOIN books host ON host.id=a.host_book_id AND host.owner_user_id=a.publisher_id
 JOIN books promoted ON promoted.id=c.promoted_book_id AND promoted.owner_user_id=c.advertiser_id
 JOIN users advertiser_account ON advertiser_account.id=c.advertiser_id AND advertiser_account.account_state='active'
 JOIN users publisher_account ON publisher_account.id=a.publisher_id AND publisher_account.account_state='active'
 JOIN rec_materializable_books visible_host ON visible_host.id=host.id
 JOIN rec_materializable_books visible_promoted ON visible_promoted.id=promoted.id
 JOIN ad_publisher_settings s ON s.user_id=a.publisher_id AND s.enabled
 WHERE host.id<>promoted.id AND a.publisher_id<>c.advertiser_id;

CREATE FUNCTION ad_reservation_evidence() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE f finance_ad_funding; previous NUMERIC; total NUMERIC; pub_previous NUMERIC; pub_total NUMERIC;
BEGIN
 SELECT * INTO f FROM finance_ad_funding WHERE intent_id=NEW.intent_id;
 previous:=floor(f.budget_minor::NUMERIC*(NEW.ordinal-1)/f.target_units);
 total:=floor(f.budget_minor::NUMERIC*NEW.ordinal/f.target_units);
 pub_previous:=floor(previous*f.publisher_share_bps/10000);
 pub_total:=floor(total*f.publisher_share_bps/10000);
 IF f.product<>'chapter' OR NEW.ordinal>f.reserved_units OR f.campaign_id<>NEW.campaign_id
   OR NEW.gross_minor<>total-previous OR NEW.publisher_minor<>pub_total-pub_previous
   OR (NEW.ordinal>1 AND NOT EXISTS(SELECT 1 FROM ad_impression_reservations r
       WHERE r.intent_id=f.intent_id AND r.ordinal=NEW.ordinal-1))
   OR NOT EXISTS(SELECT 1 FROM finance_ad_transactions t WHERE t.id=NEW.reservation_transaction_id
       AND t.intent_id=NEW.intent_id AND t.operation='reserve' AND t.amount_minor=NEW.gross_minor
       AND t.identity='ads:reserve:'||NEW.intent_id||':'||NEW.ordinal)
   OR NOT EXISTS(SELECT 1 FROM ad_viewability_opportunities o JOIN ad_delivery_assignments a ON a.id=o.assignment_id
       WHERE o.id=NEW.opportunity_id AND a.campaign_id=NEW.campaign_id AND a.publisher_id=NEW.publisher_id)
 THEN RAISE EXCEPTION 'invalid ad reservation evidence' USING ERRCODE='23514'; END IF;
 IF NEW.settled_at IS NOT NULL AND NOT EXISTS(SELECT 1 FROM finance_ad_transactions t
   JOIN ad_settlements s ON s.transaction_id=t.id AND s.reservation_id=NEW.id
   WHERE t.id=NEW.settlement_transaction_id AND t.intent_id=NEW.intent_id AND t.operation='settle'
     AND t.amount_minor=NEW.gross_minor AND t.identity='ads:settle:'||NEW.intent_id||':'||NEW.ordinal)
 THEN RAISE EXCEPTION 'invalid ad settlement evidence' USING ERRCODE='23514'; END IF;
 RETURN NULL;
END; $$;
CREATE CONSTRAINT TRIGGER ad_reservation_proof AFTER INSERT OR UPDATE ON ad_impression_reservations
 DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION ad_reservation_evidence();

CREATE TABLE ad_daily_frequency_buckets (
 campaign_id BIGINT NOT NULL REFERENCES ad_legacy_campaigns(id),
 subject_hash BYTEA NOT NULL CHECK(octet_length(subject_hash)=32),
 bucket TEXT NOT NULL CHECK(bucket IN('day','hour','session')),
 window_start TIMESTAMPTZ NOT NULL,
 count INTEGER NOT NULL CHECK(count>0),
 last_served_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
 expires_at TIMESTAMPTZ NOT NULL,
 PRIMARY KEY(campaign_id,subject_hash,bucket,window_start)
);
CREATE INDEX ad_daily_frequency_expiry ON ad_daily_frequency_buckets(expires_at);
