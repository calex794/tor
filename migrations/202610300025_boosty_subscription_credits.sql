-- D-10/S041: a verified, uniquely identified paid subscription charge may
-- credit a reader's existing wallet; NO external poller or HTTP payment entry.
-- Source truth remains the signed-in Boosty owner's financial records, not
-- subscription status, accumulated totals or client-provided operation IDs.
CREATE TABLE boosty_verified_wallet_links (
    provider_user_id BIGINT PRIMARY KEY CHECK (provider_user_id > 0),
    user_id BIGINT NOT NULL UNIQUE REFERENCES users(id) ON DELETE RESTRICT,
    verified_by TEXT NOT NULL CHECK (verified_by IN ('oauth','boosty_dm','operator')),
    verified_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- A link cannot silently be reassigned to a different reader. Explicit
-- revocation and re-verification must be a separately audited operation.
CREATE TRIGGER boosty_verified_links_immutable
    BEFORE UPDATE OR DELETE ON boosty_verified_wallet_links
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();

CREATE TABLE boosty_subscription_credits (
    provider_operation_id TEXT PRIMARY KEY
        CHECK (char_length(provider_operation_id) BETWEEN 1 AND 120),
    provider_user_id BIGINT NOT NULL
        REFERENCES boosty_verified_wallet_links(provider_user_id) ON DELETE RESTRICT,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    charge_kind TEXT NOT NULL CHECK (charge_kind IN ('subscribe','recurrent')),
    amount_minor BIGINT NOT NULL CHECK (amount_minor > 0),
    charge_fingerprint BYTEA NOT NULL CHECK (octet_length(charge_fingerprint) = 32),
    wallet_reference TEXT NOT NULL UNIQUE
        CHECK (char_length(wallet_reference) BETWEEN 8 AND 160),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX boosty_subscription_credits_by_reader
    ON boosty_subscription_credits(user_id,created_at DESC);

-- The provider claim cannot commit without its single immutable wallet entry.
CREATE FUNCTION boosty_credit_ledger_complete()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM reader_wallet_entries e
        WHERE e.user_id=NEW.user_id
          AND e.reference=NEW.wallet_reference
          AND e.kind='verified_funding'
          AND e.delta_minor=NEW.amount_minor
          AND e.purchase_id IS NULL
    ) THEN
        RAISE EXCEPTION 'Boosty charge must have a matching wallet credit'
            USING ERRCODE='23514';
    END IF;
    RETURN NULL;
END;
$$;

CREATE CONSTRAINT TRIGGER boosty_credit_ledger_complete
    AFTER INSERT ON boosty_subscription_credits
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION boosty_credit_ledger_complete();

CREATE TRIGGER boosty_subscription_credits_immutable
    BEFORE UPDATE OR DELETE ON boosty_subscription_credits
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();
