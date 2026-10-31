-- D-07: one eligible subscriber discount per owned public book.
-- Book ownership is verified by the D repository on every owner action and
-- every payment. Only a single chapter purchase can use this initial product.
CREATE TABLE author_discount_campaigns (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE RESTRICT,
    owner_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    discount_percent SMALLINT NOT NULL CHECK (discount_percent BETWEEN 1 AND 90),
    min_follow_days SMALLINT NOT NULL CHECK (min_follow_days IN (0,90)),
    starts_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    ends_at TIMESTAMPTZ NOT NULL,
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','stopped')),
    CHECK (ends_at > starts_at)
);
CREATE UNIQUE INDEX author_discount_one_active_owner_book
    ON author_discount_campaigns(book_id,owner_user_id) WHERE status='active';
CREATE INDEX author_discount_active_book
    ON author_discount_campaigns(book_id,owner_user_id,ends_at)
    WHERE status='active';

-- The financial evidence freezes the offer used for an exact, pinned purchase.
-- Discounts do not retroactively rewrite payments, beneficiary shares or refunds.
CREATE TABLE chapter_purchase_discount_snapshots (
    purchase_id BIGINT PRIMARY KEY REFERENCES chapter_purchase_facts(id) ON DELETE RESTRICT,
    campaign_id BIGINT NOT NULL REFERENCES author_discount_campaigns(id) ON DELETE RESTRICT,
    follow_period_id BIGINT NOT NULL REFERENCES creator_follow_periods(id) ON DELETE RESTRICT,
    base_price_minor BIGINT NOT NULL CHECK (base_price_minor > 0),
    discount_minor BIGINT NOT NULL CHECK (discount_minor > 0),
    paid_minor BIGINT NOT NULL CHECK (paid_minor > 0),
    discount_percent SMALLINT NOT NULL CHECK (discount_percent BETWEEN 1 AND 90),
    min_follow_days SMALLINT NOT NULL CHECK (min_follow_days IN (0,90)),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    CHECK (base_price_minor = discount_minor + paid_minor)
);
CREATE TRIGGER chapter_purchase_discount_snapshots_no_mutation
    BEFORE UPDATE OR DELETE ON chapter_purchase_discount_snapshots
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();
