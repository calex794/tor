-- S038 / FIN-02: NEwBOO-compatible unified balance reservations, NOT cash-out.
-- No public payout is enabled by this migration. No funds are debited here.
-- Capacity includes the user's verified wallet balance and held creator earnings.
-- It does not distinguish wallet deposits from creator credits as payout origins.
CREATE TABLE finance_creator_payout_requests (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    amount_minor BIGINT NOT NULL CHECK (amount_minor > 0),
    currency CHAR(3) NOT NULL DEFAULT 'RUB' CHECK (currency = 'RUB'),
    source_origin TEXT NOT NULL DEFAULT 'unified_wallet'
        CHECK (source_origin = 'unified_wallet'),
    intent_key TEXT NOT NULL CHECK (intent_key ~ '^[A-Za-z0-9_-]{43}$'),
    intent_digest BYTEA NOT NULL CHECK (octet_length(intent_digest) = 32),
    state TEXT NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending', 'rejected')),
    reservation_minor BIGINT NOT NULL CHECK (reservation_minor = amount_minor),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    decided_at TIMESTAMPTZ,
    decision_actor_id BIGINT REFERENCES users(id) ON DELETE RESTRICT,
    decision_reason TEXT,
    UNIQUE(user_id, intent_key),
    CHECK (
        (state='pending' AND decided_at IS NULL
            AND decision_actor_id IS NULL AND decision_reason IS NULL)
        OR
        (state='rejected' AND decided_at IS NOT NULL
            AND decision_actor_id IS NOT NULL
            AND char_length(decision_reason) BETWEEN 1 AND 500)
    )
);
CREATE INDEX s038_creator_payout_history
    ON finance_creator_payout_requests(user_id,id DESC);
CREATE INDEX s038_creator_payout_reserved
    ON finance_creator_payout_requests(user_id)
    WHERE state='pending';

-- Request amount/key/digest and origin cannot change. An independently
-- authorized S039 decision can ONLY reject a pending request; no internal
-- posting or external payout can be inferred from a mutable state.
CREATE FUNCTION s038_payout_evidence_immutable()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF ROW(OLD.user_id,OLD.amount_minor,OLD.currency,OLD.source_origin,
           OLD.intent_key,OLD.intent_digest,OLD.reservation_minor,OLD.created_at)
       IS DISTINCT FROM
       ROW(NEW.user_id,NEW.amount_minor,NEW.currency,NEW.source_origin,
           NEW.intent_key,NEW.intent_digest,NEW.reservation_minor,NEW.created_at)
       OR OLD.state <> 'pending' OR NEW.state <> 'rejected'
    THEN
        RAISE EXCEPTION 'S038 payout reservation evidence is immutable'
            USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER s038_payout_evidence_immutable_guard
    BEFORE UPDATE ON finance_creator_payout_requests
    FOR EACH ROW EXECUTE FUNCTION s038_payout_evidence_immutable();
CREATE TRIGGER s038_payout_no_delete
    BEFORE DELETE ON finance_creator_payout_requests
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();

COMMENT ON TABLE finance_creator_payout_requests IS
    'Unified wallet plus creator earnings capacity; pending reservations only; no banking or external settlement.';
