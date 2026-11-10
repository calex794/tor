-- S039/MONEY-03: a posted INTERNAL wallet obligation, never a bank payment.
-- No public route, provider integration or automatic cash-out is added here.
-- All state, debit, entry and posting evidence must commit or roll back together.
-- Preserve the separately generated reservation_minor CHECK. The legacy
-- table-level pending/rejected decision shape is check1, not check.
ALTER TABLE finance_creator_payout_requests
    DROP CONSTRAINT finance_creator_payout_requests_state_check,
    DROP CONSTRAINT finance_creator_payout_requests_check1,
    ADD CONSTRAINT finance_creator_payout_requests_state_check
        CHECK (state IN ('pending','rejected','posted')),
    ADD CONSTRAINT s039_payout_decision_shape CHECK (
        (state='pending' AND decided_at IS NULL
            AND decision_actor_id IS NULL AND decision_reason IS NULL)
        OR
        (state IN ('rejected','posted') AND decided_at IS NOT NULL
            AND decision_actor_id IS NOT NULL
            AND char_length(decision_reason) BETWEEN 1 AND 500)
    );

-- Preserve all existing purchase, refund, swap, Layer and creator credit kinds.
ALTER TABLE reader_wallet_entries
    DROP CONSTRAINT reader_wallet_entries_kind_check,
    DROP CONSTRAINT wallet_entry_direction,
    ADD CONSTRAINT reader_wallet_entries_kind_check CHECK (
        kind IN (
            'verified_funding','chapter_purchase','chapter_refund',
            'chapter_swap_credit','layer_purchase','creator_earning_credit',
            'creator_payout_debit'
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
        OR (kind='creator_payout_debit' AND delta_minor<0
            AND purchase_id IS NULL AND layer_term_id IS NULL)
    );

-- Reservation state transitions are monotonic, with no post-decision rewrite.
CREATE OR REPLACE FUNCTION s038_payout_evidence_immutable()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF ROW(OLD.user_id,OLD.amount_minor,OLD.currency,OLD.source_origin,
           OLD.intent_key,OLD.intent_digest,OLD.reservation_minor,OLD.created_at)
       IS DISTINCT FROM
       ROW(NEW.user_id,NEW.amount_minor,NEW.currency,NEW.source_origin,
           NEW.intent_key,NEW.intent_digest,NEW.reservation_minor,NEW.created_at)
       OR OLD.state <> 'pending' OR NEW.state NOT IN ('rejected','posted')
    THEN
        RAISE EXCEPTION 'S038 payout reservation evidence is immutable'
            USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TABLE finance_creator_payout_postings (
    payout_request_id BIGINT PRIMARY KEY
        REFERENCES finance_creator_payout_requests(id) ON DELETE RESTRICT,
    recipient_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    admin_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    amount_minor BIGINT NOT NULL CHECK (amount_minor>0),
    currency CHAR(3) NOT NULL DEFAULT 'RUB' CHECK (currency='RUB'),
    wallet_entry_id BIGINT NOT NULL UNIQUE
        REFERENCES reader_wallet_entries(id) ON DELETE RESTRICT,
    posting_reference TEXT NOT NULL UNIQUE
        CHECK (char_length(posting_reference) BETWEEN 8 AND 160),
    posted_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX s039_posting_recipient ON finance_creator_payout_postings
    (recipient_user_id,payout_request_id DESC);
CREATE TRIGGER s039_payout_postings_immutable
    BEFORE UPDATE OR DELETE ON finance_creator_payout_postings
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();

-- At commit every posting must be supported by its exact terminal request
-- AND its own immutable negative wallet journal entry.
CREATE FUNCTION s039_verify_payout_posting()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM finance_creator_payout_requests r
        JOIN reader_wallet_entries j ON j.id=NEW.wallet_entry_id
        WHERE r.id=NEW.payout_request_id
          AND r.user_id=NEW.recipient_user_id
          AND r.amount_minor=NEW.amount_minor
          AND r.currency=NEW.currency AND r.state='posted'
          AND r.decision_actor_id=NEW.admin_user_id
          AND j.user_id=NEW.recipient_user_id
          AND j.kind='creator_payout_debit'
          AND j.delta_minor=-NEW.amount_minor
          AND j.balance_after_minor>=0
          AND j.reference=NEW.posting_reference
          AND j.purchase_id IS NULL AND j.layer_term_id IS NULL
          AND NEW.posting_reference='s039:payout:'||NEW.payout_request_id::TEXT
    ) THEN
        RAISE EXCEPTION 'S039 payout posting lacks exact request and wallet debit'
            USING ERRCODE='23514';
    END IF;
    RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER s039_posting_complete
    AFTER INSERT ON finance_creator_payout_postings
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION s039_verify_payout_posting();

-- Prevent an orphan payout debit even if someone attempts to insert the
-- journal directly without the linked trusted approval.
CREATE FUNCTION s039_verify_payout_journal()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM finance_creator_payout_postings p
        WHERE p.wallet_entry_id=NEW.id AND p.recipient_user_id=NEW.user_id
          AND p.amount_minor=-NEW.delta_minor
          AND p.posting_reference=NEW.reference
    ) THEN
        RAISE EXCEPTION 'S039 orphan payout journal debit'
            USING ERRCODE='23514';
    END IF;
    RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER s039_journal_complete
    AFTER INSERT ON reader_wallet_entries
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW WHEN (NEW.kind='creator_payout_debit')
    EXECUTE FUNCTION s039_verify_payout_journal();

-- A state flip to posted without its immutable posting cannot commit.
CREATE FUNCTION s039_verify_posted_request()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM finance_creator_payout_postings p
        WHERE p.payout_request_id=NEW.id
          AND p.recipient_user_id=NEW.user_id
          AND p.admin_user_id=NEW.decision_actor_id
          AND p.amount_minor=NEW.amount_minor
    ) THEN
        RAISE EXCEPTION 'S039 posted request has no immutable posting'
            USING ERRCODE='23514';
    END IF;
    RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER s039_request_posted_complete
    AFTER UPDATE ON finance_creator_payout_requests
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW WHEN (NEW.state='posted')
    EXECUTE FUNCTION s039_verify_posted_request();

COMMENT ON TABLE finance_creator_payout_postings IS
    'Internal wallet payout debit/obligation. Posted does NOT prove a bank transfer.';

-- Widen only the reviewed session proof scopes; refund, owner-transfer and
-- revenue-share proofs retain their existing validity and 5-minute bound.
ALTER TABLE sessions DROP CONSTRAINT sessions_step_up_proof_shape;
ALTER TABLE sessions ADD CONSTRAINT sessions_step_up_proof_shape CHECK (
    (
        step_up_proof_hash IS NULL AND step_up_scope IS NULL
        AND step_up_resource_id IS NULL AND step_up_revision IS NULL
        AND step_up_payload_digest IS NULL AND step_up_issued_at IS NULL
        AND step_up_expires_at IS NULL AND step_up_used_at IS NULL
    ) OR (
        step_up_proof_hash IS NOT NULL AND octet_length(step_up_proof_hash)=32
        AND step_up_scope IN (
            'finance.refund.approve.v1',
            'book.owner.transfer.v1',
            'book.revenue.shares.v1',
            'finance.payout.post.v1'
        )
        AND step_up_resource_id>0 AND step_up_revision>0
        AND octet_length(step_up_payload_digest)=32
        AND step_up_issued_at IS NOT NULL
        AND step_up_expires_at>step_up_issued_at
        AND step_up_expires_at<=step_up_issued_at+INTERVAL '5 minutes'
        AND (step_up_used_at IS NULL OR step_up_used_at>=step_up_issued_at)
    )
);
COMMENT ON COLUMN sessions.step_up_resource_id IS
    'Request or resource ID bound to the exact session proof.';
COMMENT ON COLUMN sessions.step_up_revision IS
    'Refund/Book revision or immutable payout amount bound to the proof.';
