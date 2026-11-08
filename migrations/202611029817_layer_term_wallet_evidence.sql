-- D-05 / S031: fail closed if a paid Layer term has no matching wallet
-- journal and completed idempotent command. This is a DB linkage invariant,
-- not external cash receipt, balance reconciliation, creator earnings or payout.
-- Additive migration: historical migration checksums must remain untouched.

-- The earlier positive-duration constraint did not prove exactly 30 days:
-- PostgreSQL CHECK would also accept an UNKNOWN for a NULL expiry.
ALTER TABLE commerce_layer_terms
    ADD CONSTRAINT d05_layer_term_exact_duration CHECK (
        (term_kind='lifetime' AND expires_at IS NULL)
        OR (
            term_kind='prepaid_30d' AND expires_at IS NOT NULL
            AND expires_at=started_at+INTERVAL '30 days'
        )
    );

-- The command is claimed with term_id=NULL then assigned once, in the same
-- wallet transaction. Once assigned, all command provenance is immutable.
CREATE FUNCTION d05_guard_layer_command_history()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF (NEW.user_id,NEW.command_key,NEW.layer_id,NEW.expected_offer_id,
        NEW.expected_version_id,NEW.expected_price_minor)
       IS DISTINCT FROM
       (OLD.user_id,OLD.command_key,OLD.layer_id,OLD.expected_offer_id,
        OLD.expected_version_id,OLD.expected_price_minor)
       OR (
           OLD.term_id IS NOT NULL
           AND NEW.term_id IS DISTINCT FROM OLD.term_id
       )
    THEN
        RAISE EXCEPTION 'Layer checkout command history is immutable'
            USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER d05_layer_command_history_guard
    BEFORE UPDATE ON commerce_layer_commands
    FOR EACH ROW EXECUTE FUNCTION d05_guard_layer_command_history();

-- Validate the term, the originally quoted values, one unique debit, and
-- the deterministic journal reference. A negative journal row by itself is
-- NOT proof of external settlement; creator posting remains separately gated.
CREATE FUNCTION d05_layer_term_has_wallet_evidence(p_term_id BIGINT)
RETURNS BOOLEAN LANGUAGE sql STABLE AS $$
    SELECT EXISTS (
        SELECT 1
        FROM commerce_layer_terms t
        JOIN commerce_layer_commands c
          ON c.term_id=t.id
         AND c.user_id=t.user_id
         AND c.layer_id=t.layer_id
         AND c.expected_offer_id=t.offer_id
         AND c.expected_version_id=t.base_version_id
         AND c.expected_price_minor=t.amount_minor
        JOIN commerce_layer_offers o
          ON o.id=t.offer_id
         AND o.layer_id=t.layer_id
         AND o.price_minor=t.amount_minor
         AND o.term_kind=t.term_kind
         AND o.currency='RUB'
        JOIN reader_wallet_entries e
          ON e.layer_term_id=t.id
         AND e.user_id=t.user_id
         AND e.kind='layer_purchase'
         AND e.delta_minor=-t.amount_minor
         AND e.reference='layer-wallet:'||t.user_id::TEXT||':'||c.command_key
        JOIN reader_wallet_accounts w
          ON w.user_id=t.user_id AND w.currency='RUB'
        WHERE t.id=p_term_id
    );
$$;

-- Checkout inserts the term first, then snapshots, wallet entry, and command
-- result. Deferred evaluation sees all of them or rolls the entire unit back.
CREATE FUNCTION d05_verify_layer_term_wallet_evidence()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT d05_layer_term_has_wallet_evidence(NEW.id) THEN
        RAISE EXCEPTION 'paid Layer term lacks matching wallet and command evidence'
            USING ERRCODE='23514';
    END IF;
    RETURN NULL;
END;
$$;

CREATE CONSTRAINT TRIGGER d05_layer_term_wallet_evidence
    AFTER INSERT ON commerce_layer_terms
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION d05_verify_layer_term_wallet_evidence();

-- An upgrade cannot quietly grandfather an orphaned entitlement. The CHECK
-- above validates historical durations; this scan validates linked evidence.
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM commerce_layer_terms t
        WHERE NOT d05_layer_term_has_wallet_evidence(t.id)
    ) THEN
        RAISE EXCEPTION 'historical Layer terms lack wallet/command evidence'
            USING ERRCODE='23514';
    END IF;
END;
$$;
