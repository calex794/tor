-- D-07: one eligible subscriber discount per owned public book.
-- Book ownership is verified by the D repository on every owner action and
-- every payment. Only a single chapter purchase can use this initial product.
CREATE TABLE author_discount_campaigns (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE RESTRICT,
    owner_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    discount_percent SMALLINT NOT NULL CHECK (discount_percent BETWEEN 1 AND 90),
    min_follow_days SMALLINT NOT NULL CHECK (min_follow_days IN (0,30,90,180)),
    starts_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    ends_at TIMESTAMPTZ NOT NULL,
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','stopped')),
    revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
    CHECK (ends_at > starts_at)
);
CREATE UNIQUE INDEX author_discount_one_active_owner_book
    ON author_discount_campaigns(book_id,owner_user_id) WHERE status='active';
CREATE INDEX author_discount_active_book
    ON author_discount_campaigns(book_id,owner_user_id,ends_at)
    WHERE status='active';

-- Any change to the quoted terms advances a durable revision. Stopping a
-- campaign does not rewrite the terms a reader previously accepted.
CREATE FUNCTION bump_author_discount_revision()
RETURNS trigger LANGUAGE plpgsql AS $bump_author_discount_revision$
BEGIN
    IF ROW(NEW.book_id,NEW.owner_user_id,NEW.discount_percent,NEW.min_follow_days,
           NEW.starts_at,NEW.ends_at)
       IS DISTINCT FROM
       ROW(OLD.book_id,OLD.owner_user_id,OLD.discount_percent,OLD.min_follow_days,
           OLD.starts_at,OLD.ends_at) THEN
        NEW.revision := OLD.revision + 1;
    ELSE
        NEW.revision := OLD.revision;
    END IF;
    RETURN NEW;
END; $bump_author_discount_revision$;
CREATE TRIGGER author_discount_revision
    BEFORE UPDATE ON author_discount_campaigns
    FOR EACH ROW EXECUTE FUNCTION bump_author_discount_revision();

-- Bind every wallet command to both list and final price, and to the exact
-- campaign/continuous-follow period shown to the reader. Existing D-02
-- commands were full-price offers, so their list price equals their price.
ALTER TABLE chapter_purchase_commands
    ADD COLUMN expected_base_price_minor BIGINT,
    ADD COLUMN expected_campaign_id BIGINT
        REFERENCES author_discount_campaigns(id) ON DELETE RESTRICT,
    ADD COLUMN expected_campaign_revision INTEGER,
    ADD COLUMN expected_rule_version SMALLINT,
    ADD COLUMN expected_follow_period_id BIGINT
        REFERENCES creator_follow_periods(id) ON DELETE RESTRICT,
    ADD COLUMN expected_discount_percent SMALLINT,
    ADD COLUMN expected_min_follow_days SMALLINT;

UPDATE chapter_purchase_commands
SET expected_base_price_minor=expected_price_minor
WHERE expected_base_price_minor IS NULL;

ALTER TABLE chapter_purchase_commands
    ALTER COLUMN expected_base_price_minor SET NOT NULL,
    ADD CONSTRAINT chapter_purchase_commands_base_price_positive
        CHECK (expected_base_price_minor > 0),
    ADD CONSTRAINT chapter_purchase_commands_price_within_base
        CHECK (expected_base_price_minor >= expected_price_minor),
    ADD CONSTRAINT chapter_purchase_commands_quote_fingerprint
        CHECK (
            (expected_campaign_id IS NULL
             AND expected_campaign_revision IS NULL
             AND expected_rule_version IS NULL
             AND expected_follow_period_id IS NULL
             AND expected_discount_percent IS NULL
             AND expected_min_follow_days IS NULL
             AND expected_base_price_minor=expected_price_minor)
            OR
            (expected_campaign_id IS NOT NULL
             AND expected_campaign_revision IS NOT NULL
             AND expected_campaign_revision > 0
             AND expected_rule_version IS NOT NULL
             AND expected_rule_version > 0
             AND expected_follow_period_id IS NOT NULL
             AND expected_discount_percent IS NOT NULL
             AND expected_discount_percent BETWEEN 1 AND 90
             AND expected_min_follow_days IS NOT NULL
             AND expected_min_follow_days IN (0,30,90,180)
             AND expected_base_price_minor > expected_price_minor)
        );

-- The financial evidence freezes the offer used for an exact, pinned purchase.
-- Discounts do not retroactively rewrite payments, beneficiary shares or refunds.
CREATE TABLE chapter_purchase_discount_snapshots (
    purchase_id BIGINT PRIMARY KEY REFERENCES chapter_purchase_facts(id) ON DELETE RESTRICT,
    campaign_id BIGINT NOT NULL REFERENCES author_discount_campaigns(id) ON DELETE RESTRICT,
    campaign_revision INTEGER NOT NULL CHECK (campaign_revision > 0),
    rule_version SMALLINT NOT NULL CHECK (rule_version > 0),
    follow_period_id BIGINT NOT NULL REFERENCES creator_follow_periods(id) ON DELETE RESTRICT,
    base_price_minor BIGINT NOT NULL CHECK (base_price_minor > 0),
    discount_minor BIGINT NOT NULL CHECK (discount_minor > 0),
    paid_minor BIGINT NOT NULL CHECK (paid_minor > 0),
    discount_percent SMALLINT NOT NULL CHECK (discount_percent BETWEEN 1 AND 90),
    min_follow_days SMALLINT NOT NULL CHECK (min_follow_days IN (0,30,90,180)),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    CHECK (base_price_minor = discount_minor + paid_minor)
);
CREATE TRIGGER chapter_purchase_discount_snapshots_no_mutation
    BEFORE UPDATE OR DELETE ON chapter_purchase_discount_snapshots
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();
