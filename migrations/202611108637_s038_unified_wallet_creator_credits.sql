-- S038 / FIN-02b: unify spendable wallet and mature creator credits without
-- reclassifying the source of wallet top-ups. NO automatic release/payout.
--
-- The immutable creator earning remains the origin and is counted in the
-- S038 read projection only until this exact credit is journaled once.
-- Preserve ALL accepted wallet journal kinds from D-06 swaps and D-05 layers.
-- In particular never remove the layer_term_id evidence predicate.
ALTER TABLE reader_wallet_entries
    DROP CONSTRAINT reader_wallet_entries_kind_check,
    DROP CONSTRAINT wallet_entry_direction,
    ADD CONSTRAINT reader_wallet_entries_kind_check CHECK (
        kind IN (
            'verified_funding', 'chapter_purchase', 'chapter_refund',
            'chapter_swap_credit', 'layer_purchase', 'creator_earning_credit'
        )
    ),
    ADD CONSTRAINT wallet_entry_direction CHECK (
        (kind='verified_funding' AND delta_minor>0
            AND purchase_id IS NULL AND layer_term_id IS NULL)
        OR (kind='chapter_purchase' AND delta_minor<0
            AND purchase_id IS NOT NULL AND layer_term_id IS NULL)
        OR (kind='chapter_refund' AND delta_minor>0
            AND purchase_id IS NOT NULL AND layer_term_id IS NULL)
        OR (kind='chapter_swap_credit' AND delta_minor>0
            AND purchase_id IS NOT NULL AND layer_term_id IS NULL)
        OR (kind='layer_purchase' AND delta_minor<0
            AND purchase_id IS NULL AND layer_term_id IS NOT NULL)
        OR (kind='creator_earning_credit' AND delta_minor>0
            AND purchase_id IS NULL AND layer_term_id IS NULL)
    );

CREATE TABLE finance_creator_wallet_credits (
    purchase_id BIGINT NOT NULL,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    amount_minor BIGINT NOT NULL CHECK (amount_minor>0),
    wallet_entry_id BIGINT NOT NULL UNIQUE
        REFERENCES reader_wallet_entries(id) ON DELETE RESTRICT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY(purchase_id,user_id),
    FOREIGN KEY(purchase_id,user_id)
        REFERENCES finance_creator_earnings(purchase_id,user_id)
        ON DELETE RESTRICT
);
CREATE INDEX s038_wallet_credits_by_user
    ON finance_creator_wallet_credits(user_id,purchase_id);

CREATE TRIGGER s038_wallet_credit_immutable
    BEFORE UPDATE OR DELETE ON finance_creator_wallet_credits
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();

-- Strict bidirectional evidence checks: the posting and the wallet journal
-- must match the exact immutable earning and one another AT COMMIT.
CREATE FUNCTION s038_verify_creator_wallet_credit()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM finance_creator_earnings e
        JOIN reader_wallet_entries j ON j.id=NEW.wallet_entry_id
        JOIN finance_chapter_postings p ON p.purchase_id=e.purchase_id
        WHERE e.purchase_id=NEW.purchase_id AND e.user_id=NEW.user_id
          AND e.origin_type='chapter_purchase'
          AND e.amount_minor=NEW.amount_minor
          AND e.clear_at<=NEW.created_at
          AND p.refund_deadline_at<=NEW.created_at
          AND j.user_id=NEW.user_id
          AND j.kind='creator_earning_credit'
          AND j.delta_minor=NEW.amount_minor
          AND j.purchase_id IS NULL
          AND j.reference=('creator-credit:' || NEW.purchase_id || ':' || NEW.user_id)
          AND j.created_at<=NEW.created_at
          AND NOT EXISTS (
              SELECT 1 FROM finance_direct_refund_requests q
              WHERE q.purchase_id=NEW.purchase_id
                AND q.state IN ('pending','processing','approved')
          )
          AND NOT EXISTS (
              SELECT 1 FROM finance_direct_refund_creator_reversals r
              WHERE r.purchase_id=NEW.purchase_id AND r.user_id=NEW.user_id
          )
    ) THEN
        RAISE EXCEPTION 'creator wallet credit lacks mature verified earning'
            USING ERRCODE='23514';
    END IF;
    RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER s038_creator_wallet_credit_complete
    AFTER INSERT ON finance_creator_wallet_credits
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION s038_verify_creator_wallet_credit();

CREATE FUNCTION s038_verify_credit_entry_has_origin()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS(
        SELECT 1 FROM finance_creator_wallet_credits c
        WHERE c.wallet_entry_id=NEW.id
          AND c.user_id=NEW.user_id
          AND c.amount_minor=NEW.delta_minor
    ) THEN
        RAISE EXCEPTION 'orphan creator wallet credit'
            USING ERRCODE='23514';
    END IF;
    RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER s038_creator_wallet_entry_complete
    AFTER INSERT ON reader_wallet_entries
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW WHEN (NEW.kind='creator_earning_credit')
    EXECUTE FUNCTION s038_verify_credit_entry_has_origin();

-- A payout reservation and a wallet debit MUST NOT spend the same money.
-- A debit first holds the wallet row. The trigger also locks the user's row,
-- exactly the lock acquired first by PgCreatorFinance::reserve_once. A
-- concurrent reservation then uses a fresh READ COMMITTED snapshot.
-- Any negative net from a later creator reversal is treated as debt.
CREATE FUNCTION s038_wallet_debit_reservation_guard()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE
    mature_unposted BIGINT;
    refund_hold_total BIGINT;
    active_reservations BIGINT;
BEGIN
    IF NEW.balance_minor>=OLD.balance_minor THEN
        RETURN NEW;
    END IF;
    PERFORM 1 FROM users WHERE id=NEW.user_id FOR UPDATE;
    WITH net AS (
        SELECT e.purchase_id,
               e.clear_at,
               (e.amount_minor-COALESCE(r.amount_minor,0)
                              -COALESCE(c.amount_minor,0))::BIGINT AS unposted,
               EXISTS (
                   SELECT 1 FROM finance_direct_refund_requests q
                   WHERE q.purchase_id=e.purchase_id
                     AND q.state IN ('pending','processing','approved')
                     AND r.purchase_id IS NULL
               ) AS refund_hold
        FROM finance_creator_earnings e
        LEFT JOIN finance_direct_refund_creator_reversals r
          ON r.purchase_id=e.purchase_id AND r.user_id=e.user_id
        LEFT JOIN finance_creator_wallet_credits c
          ON c.purchase_id=e.purchase_id AND c.user_id=e.user_id
        WHERE e.user_id=NEW.user_id AND e.origin_type='chapter_purchase'
    )
    SELECT COALESCE(SUM(CASE WHEN clear_at<=clock_timestamp()
                             THEN unposted ELSE 0 END),0)::BIGINT,
           COALESCE(SUM(CASE WHEN clear_at<=clock_timestamp()
                                   AND refund_hold
                              THEN GREATEST(unposted,0) ELSE 0 END),0)::BIGINT
    INTO mature_unposted,refund_hold_total FROM net;

    SELECT COALESCE(SUM(reservation_minor),0)::BIGINT
    INTO active_reservations FROM finance_creator_payout_requests
    WHERE user_id=NEW.user_id AND state='pending';

    IF NEW.balance_minor + mature_unposted - refund_hold_total < active_reservations THEN
        RAISE EXCEPTION 'wallet debit would consume reserved payout or clawback debt'
            USING ERRCODE='P0R38';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER s038_wallet_debit_reservation_guard
    BEFORE UPDATE OF balance_minor ON reader_wallet_accounts
    FOR EACH ROW EXECUTE FUNCTION s038_wallet_debit_reservation_guard();

COMMENT ON TABLE finance_creator_wallet_credits IS
    'Immutable one-time wallet credit backed by mature creator earnings; no external payouts.';
