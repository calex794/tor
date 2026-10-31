-- D-08 / S038 / FIN-02: opt-in, exact creator accounting.
-- Do not seed an unapproved platform commission or refund policy. An operator
-- must publish versioned book terms before enabling creator posting.
-- The existing buyer-wallet-only checkout remains untouched until explicitly
-- enabled; no external payout or cash-out route is added here.

CREATE TABLE finance_revenue_terms (
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE RESTRICT,
    policy_version TEXT NOT NULL CHECK (
        policy_version ~ '^[A-Za-z0-9][A-Za-z0-9_.:-]{0,79}$'
    ),
    platform_fee_bps SMALLINT NOT NULL
        CHECK (platform_fee_bps BETWEEN 0 AND 10000),
    refund_policy_version TEXT NOT NULL CHECK (
        refund_policy_version ~ '^[A-Za-z0-9][A-Za-z0-9_.:-]{0,79}$'
    ),
    refund_window_seconds BIGINT NOT NULL
        CHECK (refund_window_seconds BETWEEN 1 AND 7776000),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (book_id, policy_version)
);
CREATE TRIGGER finance_revenue_terms_immutable
    BEFORE UPDATE OR DELETE ON finance_revenue_terms
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();

-- Mutable pointer only: old terms and purchase postings never change.
-- Policy changes are an operator-controlled action, not a public endpoint.
CREATE TABLE finance_revenue_term_states (
    book_id BIGINT PRIMARY KEY REFERENCES books(id) ON DELETE RESTRICT,
    policy_version TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (book_id, policy_version)
        REFERENCES finance_revenue_terms(book_id, policy_version) ON DELETE RESTRICT
);

-- These optional fields exist only for NEW policy-backed purchases.
-- Old purchases are not assigned an invented refund window.
ALTER TABLE chapter_purchase_facts
    ADD COLUMN refund_policy_version TEXT,
    ADD COLUMN refund_deadline_at TIMESTAMPTZ,
    ADD CONSTRAINT d08_refund_policy_pair CHECK (
        (refund_policy_version IS NULL AND refund_deadline_at IS NULL)
        OR (refund_policy_version IS NOT NULL AND refund_deadline_at IS NOT NULL)
    );

-- One immutable finance authority per purchase. There are no cash-out buttons
-- and no writer to reader_wallet_accounts for creators: their cash-origin
-- earnings are distinct from the buyer's spendable internal wallet.
CREATE TABLE finance_chapter_postings (
    purchase_id BIGINT PRIMARY KEY
        REFERENCES chapter_purchase_facts(id) ON DELETE RESTRICT,
    book_id BIGINT NOT NULL,
    share_revision BIGINT NOT NULL,
    policy_version TEXT NOT NULL,
    refund_policy_version TEXT NOT NULL,
    refund_deadline_at TIMESTAMPTZ NOT NULL,
    gross_minor BIGINT NOT NULL CHECK (gross_minor > 0),
    platform_fee_minor BIGINT NOT NULL CHECK (platform_fee_minor >= 0),
    creator_pool_minor BIGINT NOT NULL CHECK (creator_pool_minor >= 0),
    currency CHAR(3) NOT NULL DEFAULT 'RUB' CHECK (currency = 'RUB'),
    origin_type TEXT NOT NULL DEFAULT 'chapter_purchase'
        CHECK (origin_type = 'chapter_purchase'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (gross_minor = platform_fee_minor + creator_pool_minor),
    FOREIGN KEY (book_id, share_revision)
        REFERENCES revenue_share_versions(book_id, revision) ON DELETE RESTRICT,
    FOREIGN KEY (book_id, policy_version)
        REFERENCES finance_revenue_terms(book_id, policy_version) ON DELETE RESTRICT
);

-- The creator credit and its refundable hold are ONE authoritative record.
-- Until refund_deadline_at it cannot be counted as mature cash earnings.
-- Future reversals append a separate disposition; they cannot erase this fact.
CREATE TABLE finance_creator_earnings (
    purchase_id BIGINT NOT NULL
        REFERENCES finance_chapter_postings(purchase_id) ON DELETE RESTRICT,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    share_percent SMALLINT NOT NULL CHECK (share_percent BETWEEN 1 AND 100),
    amount_minor BIGINT NOT NULL CHECK (amount_minor > 0),
    clear_at TIMESTAMPTZ NOT NULL,
    origin_type TEXT NOT NULL DEFAULT 'chapter_purchase'
        CHECK (origin_type = 'chapter_purchase'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (purchase_id, user_id),
    FOREIGN KEY (purchase_id, user_id)
        REFERENCES chapter_purchase_share_lines(purchase_id, user_id)
        ON DELETE RESTRICT
);
CREATE INDEX finance_creator_earnings_capacity
    ON finance_creator_earnings (user_id, clear_at, purchase_id);

CREATE TABLE finance_platform_credits (
    purchase_id BIGINT PRIMARY KEY
        REFERENCES finance_chapter_postings(purchase_id) ON DELETE RESTRICT,
    amount_minor BIGINT NOT NULL CHECK (amount_minor >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Only the active term pointer may change. Finance history is append-only.
CREATE TRIGGER finance_chapter_postings_immutable
    BEFORE UPDATE OR DELETE ON finance_chapter_postings
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();
CREATE TRIGGER finance_creator_earnings_immutable
    BEFORE UPDATE OR DELETE ON finance_creator_earnings
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();
CREATE TRIGGER finance_platform_credits_immutable
    BEFORE UPDATE OR DELETE ON finance_platform_credits
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();

-- At COMMIT compare the exact saved purchase, buyer debit, platform credit
-- and EVERY creator allocation with the immutable B-10 share snapshot.
-- A missing or altered hold aborts the *same* transaction as buyer debit and
-- library grant. The SQL mirrors the Rust largest-remainder algorithm.
CREATE FUNCTION d08_verify_atomic_finance()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE
    invalid_lines BIGINT;
    platform_credit BIGINT;
    buyer_debits BIGINT;
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM chapter_purchase_facts p
        JOIN chapter_purchase_share_snapshots s ON s.purchase_id=p.id
        WHERE p.id=NEW.purchase_id
          AND p.amount_minor=NEW.gross_minor
          AND p.currency='RUB'
          AND p.refund_policy_version=NEW.refund_policy_version
          AND p.refund_deadline_at=NEW.refund_deadline_at
          AND p.refund_deadline_at > p.acquired_at
          AND s.book_id=NEW.book_id
          AND s.share_revision=NEW.share_revision
          AND s.gross_amount_minor=NEW.gross_minor
    ) THEN
        RAISE EXCEPTION 'finance posting does not match immutable purchase'
            USING ERRCODE='23514';
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM finance_revenue_terms t
        JOIN chapter_purchase_facts p ON p.id=NEW.purchase_id
        WHERE t.book_id=NEW.book_id AND t.policy_version=NEW.policy_version
          AND t.refund_policy_version=NEW.refund_policy_version
          AND p.refund_deadline_at =
              p.acquired_at + (t.refund_window_seconds * INTERVAL '1 second')
          AND NEW.platform_fee_minor =
              floor((NEW.gross_minor::numeric * t.platform_fee_bps + 5000)/10000)::BIGINT
    ) THEN
        RAISE EXCEPTION 'finance posting does not match published terms'
            USING ERRCODE='23514';
    END IF;

    SELECT amount_minor INTO platform_credit
    FROM finance_platform_credits WHERE purchase_id=NEW.purchase_id;
    IF platform_credit IS DISTINCT FROM NEW.platform_fee_minor THEN
        RAISE EXCEPTION 'platform credit missing or inconsistent'
            USING ERRCODE='23514';
    END IF;

    SELECT COUNT(*) INTO buyer_debits
    FROM reader_wallet_entries e
    JOIN chapter_purchase_facts p ON p.id=e.purchase_id
    WHERE e.purchase_id=NEW.purchase_id AND e.user_id=p.user_id
      AND e.kind='chapter_purchase' AND e.delta_minor=-NEW.gross_minor;
    IF buyer_debits <> 1 THEN
        RAISE EXCEPTION 'finance posting lacks exact buyer debit'
            USING ERRCODE='23514';
    END IF;

    WITH parts AS (
        SELECT l.user_id, l.share_percent,
            floor(NEW.creator_pool_minor::numeric * l.share_percent / 100)::BIGINT AS base_minor,
            mod(NEW.creator_pool_minor::numeric * l.share_percent, 100) AS remainder
        FROM chapter_purchase_share_lines l
        WHERE l.purchase_id=NEW.purchase_id
    ),
    ranked AS (
        SELECT user_id, share_percent, base_minor,
            row_number() OVER (ORDER BY remainder DESC, user_id) AS priority,
            SUM(base_minor) OVER () AS distributed
        FROM parts
    ),
    exact AS (
        SELECT user_id,share_percent,
            base_minor +
                CASE WHEN priority <= NEW.creator_pool_minor - distributed
                    THEN 1 ELSE 0 END AS expected_minor
        FROM ranked
    )
    SELECT COUNT(*) INTO invalid_lines
    FROM exact x
    FULL OUTER JOIN finance_creator_earnings e
      ON e.purchase_id=NEW.purchase_id AND e.user_id=x.user_id
    WHERE x.user_id IS NULL
       OR (x.expected_minor=0 AND e.user_id IS NOT NULL)
       OR (x.expected_minor>0 AND (
          e.user_id IS NULL
          OR e.amount_minor IS DISTINCT FROM x.expected_minor
          OR e.share_percent IS DISTINCT FROM x.share_percent
          OR e.clear_at IS DISTINCT FROM NEW.refund_deadline_at
       ));

    IF invalid_lines <> 0 THEN
        RAISE EXCEPTION 'creator credit/hold differs from exact frozen allocation'
            USING ERRCODE='23514';
    END IF;
    RETURN NULL;
END;
$$;

CREATE CONSTRAINT TRIGGER d08_atomic_finance_complete
    AFTER INSERT ON finance_chapter_postings
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION d08_verify_atomic_finance();
