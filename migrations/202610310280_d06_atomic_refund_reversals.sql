-- D-06 / BR-038: exact, append-only internal refund settlement.
-- No public refund approval endpoint, no external transfer, no cash-out.
-- A refund is legal only for a D-08 purchase with an approved D-06 policy,
-- first-grant tracking, no protected exposure and a non-expired deadline.
-- PostgreSQL independently verifies the money/evidence at COMMIT.

-- Existing buyer wallet has only funding/purchase: add an explicit internal
-- refund credit, never repurpose verified_funding or change an old debit.
ALTER TABLE reader_wallet_entries
    DROP CONSTRAINT reader_wallet_entries_kind_check;
ALTER TABLE reader_wallet_entries
    ADD CONSTRAINT reader_wallet_entries_kind_check
        CHECK (kind IN ('verified_funding','chapter_purchase','chapter_refund'));
ALTER TABLE reader_wallet_entries
    DROP CONSTRAINT wallet_entry_direction;
ALTER TABLE reader_wallet_entries
    ADD CONSTRAINT wallet_entry_direction CHECK (
        (kind='chapter_purchase' AND delta_minor < 0 AND purchase_id IS NOT NULL)
        OR (kind='verified_funding' AND delta_minor > 0 AND purchase_id IS NULL)
        OR (kind='chapter_refund' AND delta_minor > 0 AND purchase_id IS NOT NULL)
    );

-- A buyer may receive at most one internal refund credit for one purchase.
CREATE UNIQUE INDEX d06_unique_buyer_wallet_refund_per_purchase
    ON reader_wallet_entries(purchase_id) WHERE kind='chapter_refund';

-- Cross-check refund request identity, not just its ID.
ALTER TABLE finance_direct_refund_requests
    ADD CONSTRAINT d06_refund_request_identity UNIQUE(id,purchase_id,user_id);

CREATE TABLE finance_direct_refund_postings (
    purchase_id BIGINT PRIMARY KEY
        REFERENCES finance_chapter_postings(purchase_id) ON DELETE RESTRICT,
    request_id BIGINT NOT NULL UNIQUE,
    buyer_user_id BIGINT NOT NULL REFERENCES reader_wallet_accounts(user_id) ON DELETE RESTRICT,
    amount_minor BIGINT NOT NULL CHECK (amount_minor > 0),
    platform_reversal_minor BIGINT NOT NULL CHECK (platform_reversal_minor >= 0),
    creator_reversal_minor BIGINT NOT NULL CHECK (creator_reversal_minor >= 0),
    currency CHAR(3) NOT NULL DEFAULT 'RUB' CHECK (currency='RUB'),
    reversal_reference TEXT NOT NULL UNIQUE
        CHECK (char_length(reversal_reference) BETWEEN 8 AND 160),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    FOREIGN KEY(request_id,purchase_id,buyer_user_id)
        REFERENCES finance_direct_refund_requests(id,purchase_id,user_id)
        ON DELETE RESTRICT,
    CHECK (amount_minor=platform_reversal_minor+creator_reversal_minor)
);

-- Credit-reversal of original recipient amounts, not a recomputation from
-- today's book roles, current revenue shares, or wallet spend.
CREATE TABLE finance_direct_refund_creator_reversals (
    purchase_id BIGINT NOT NULL
        REFERENCES finance_direct_refund_postings(purchase_id) ON DELETE RESTRICT,
    user_id BIGINT NOT NULL,
    amount_minor BIGINT NOT NULL CHECK (amount_minor > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY(purchase_id,user_id),
    FOREIGN KEY(purchase_id,user_id)
        REFERENCES finance_creator_earnings(purchase_id,user_id) ON DELETE RESTRICT
);
CREATE INDEX d06_creator_refunds_by_recipient
    ON finance_direct_refund_creator_reversals(user_id,purchase_id);

CREATE TRIGGER d06_direct_refund_postings_immutable
    BEFORE UPDATE OR DELETE ON finance_direct_refund_postings
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();
CREATE TRIGGER d06_direct_refund_creator_reversals_immutable
    BEFORE UPDATE OR DELETE ON finance_direct_refund_creator_reversals
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();

-- A forged refund header cannot commit without the exact approved request,
-- first-grant barrier, entitlement revocation, buyer credit and each creator
-- reversal. A failure of ANY step aborts the entire financial transaction.
CREATE FUNCTION d06_verify_atomic_refund()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE
    bad_lines BIGINT;
    matching_credits BIGINT;
    balance_after BIGINT;
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM chapter_purchase_facts p
        JOIN finance_chapter_postings fp ON fp.purchase_id=p.id
        JOIN finance_platform_credits pc ON pc.purchase_id=p.id
        JOIN finance_direct_refund_requests q
          ON q.id=NEW.request_id AND q.purchase_id=p.id AND q.user_id=p.user_id
        JOIN finance_direct_refund_policy_catalog policy
          ON policy.version=p.refund_policy_version
        WHERE p.id=NEW.purchase_id AND p.user_id=NEW.buyer_user_id
          AND p.amount_minor=NEW.amount_minor
          AND p.currency='RUB'
          AND fp.currency='RUB'
          AND fp.gross_minor=NEW.amount_minor
          AND fp.platform_fee_minor=NEW.platform_reversal_minor
          AND fp.creator_pool_minor=NEW.creator_reversal_minor
          AND pc.amount_minor=NEW.platform_reversal_minor
          AND fp.refund_policy_version=p.refund_policy_version
          AND fp.refund_deadline_at=p.refund_deadline_at
          AND p.first_grant_tracking_version='c06-first-issue-v1'
          AND policy.approved_at<=p.acquired_at
          AND policy.effective_at<=p.acquired_at
          AND p.refund_deadline_at <= p.acquired_at
              + (policy.window_seconds * INTERVAL '1 second')
          AND q.state='approved'
          AND q.decided_at IS NOT NULL
          AND q.decided_at <= p.refund_deadline_at
          AND q.decision_actor_id IS NOT NULL
          AND q.decision_reference=NEW.reversal_reference
          AND NOT EXISTS (
              SELECT 1 FROM chapter_purchase_first_exposures e
              WHERE e.purchase_id=p.id
          )
          AND EXISTS (
              SELECT 1 FROM chapter_purchase_revocations r
              WHERE r.purchase_id=p.id AND r.reason='refund'
                AND r.reversal_reference=NEW.reversal_reference
          )
          AND (SELECT COUNT(*) FROM reader_wallet_entries orig
               WHERE orig.purchase_id=p.id AND orig.user_id=p.user_id
                 AND orig.kind='chapter_purchase'
                 AND orig.delta_minor=-p.amount_minor
                 AND orig.reference=p.settlement_reference)=1
    ) THEN
        RAISE EXCEPTION 'refund lacks approved policy, purchase, revocation or barrier'
          USING ERRCODE='23514';
    END IF;

    SELECT COUNT(*), MAX(e.balance_after_minor)
       INTO matching_credits,balance_after
    FROM reader_wallet_entries e
    WHERE e.purchase_id=NEW.purchase_id
      AND e.user_id=NEW.buyer_user_id
      AND e.kind='chapter_refund'
      AND e.delta_minor=NEW.amount_minor
      AND e.reference=NEW.reversal_reference;
    IF matching_credits<>1 OR NOT EXISTS (
        SELECT 1 FROM reader_wallet_accounts a
        WHERE a.user_id=NEW.buyer_user_id AND a.currency='RUB'
          AND a.balance_minor=balance_after
    ) THEN
        RAISE EXCEPTION 'refund buyer wallet credit is incomplete'
          USING ERRCODE='23514';
    END IF;

    -- Compare every original contributor with the exact reversal, not just
    -- SUM: a recipient cannot be silently skipped or overpaid.
    SELECT COUNT(*) INTO bad_lines
    FROM (
        SELECT user_id,amount_minor
        FROM finance_creator_earnings WHERE purchase_id=NEW.purchase_id
    ) original
    FULL OUTER JOIN (
        SELECT user_id,amount_minor
        FROM finance_direct_refund_creator_reversals
        WHERE purchase_id=NEW.purchase_id
    ) reverse ON reverse.user_id=original.user_id
    WHERE original.user_id IS NULL
       OR reverse.user_id IS NULL
       OR original.amount_minor IS DISTINCT FROM reverse.amount_minor;
    IF bad_lines<>0 OR
       (SELECT COALESCE(SUM(amount_minor),0)
        FROM finance_direct_refund_creator_reversals
        WHERE purchase_id=NEW.purchase_id)<>NEW.creator_reversal_minor
    THEN
        RAISE EXCEPTION 'creator reversal is missing or mismatched'
          USING ERRCODE='23514';
    END IF;
    RETURN NULL;
END;
$$;

CREATE CONSTRAINT TRIGGER d06_atomic_refund_complete
    AFTER INSERT ON finance_direct_refund_postings
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION d06_verify_atomic_refund();

-- The reverse invariant also holds: a standalone wallet refund credit cannot
-- commit without its corresponding complete immutable refund posting.
CREATE FUNCTION d06_verify_refund_credit_has_posting()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM finance_direct_refund_postings r
        WHERE r.purchase_id=NEW.purchase_id
          AND r.buyer_user_id=NEW.user_id
          AND r.amount_minor=NEW.delta_minor
          AND r.reversal_reference=NEW.reference
    ) THEN
        RAISE EXCEPTION 'orphan internal refund wallet credit'
          USING ERRCODE='23514';
    END IF;
    RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER d06_refund_wallet_credit_complete
    AFTER INSERT ON reader_wallet_entries
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW WHEN (NEW.kind='chapter_refund')
    EXECUTE FUNCTION d06_verify_refund_credit_has_posting();
