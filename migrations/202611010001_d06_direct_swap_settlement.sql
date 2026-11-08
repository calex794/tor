-- D-06: private, atomic swap of two direct paid chapter purchases.
-- No public route is enabled. The original purchase and accounting remain
-- immutable; the source is revoked and the target is a separate D-08 sale.
-- No policy or legal entitlement is inferred for historical purchases.

ALTER TABLE chapter_purchase_revocations
    DROP CONSTRAINT chapter_purchase_revocations_reason_check;
ALTER TABLE chapter_purchase_revocations
    ADD CONSTRAINT chapter_purchase_revocations_reason_check
    CHECK (reason IN ('refund','chargeback','fraud','purchase_swap'));

ALTER TABLE reader_wallet_entries
    DROP CONSTRAINT reader_wallet_entries_kind_check;
ALTER TABLE reader_wallet_entries
    ADD CONSTRAINT reader_wallet_entries_kind_check
    CHECK (kind IN ('verified_funding','chapter_purchase','chapter_refund','chapter_swap_credit'));
ALTER TABLE reader_wallet_entries
    DROP CONSTRAINT wallet_entry_direction;
ALTER TABLE reader_wallet_entries
    ADD CONSTRAINT wallet_entry_direction CHECK (
      (kind='chapter_purchase' AND delta_minor<0 AND purchase_id IS NOT NULL)
      OR (kind='verified_funding' AND delta_minor>0 AND purchase_id IS NULL)
      OR (kind='chapter_refund' AND delta_minor>0 AND purchase_id IS NOT NULL)
      OR (kind='chapter_swap_credit' AND delta_minor>0 AND purchase_id IS NOT NULL)
    );
CREATE UNIQUE INDEX d06_one_swap_wallet_credit
    ON reader_wallet_entries(purchase_id) WHERE kind='chapter_swap_credit';

-- The claim is scoped to buyer + request key. Reuse with different intent
-- cannot change the original row; incomplete claims cannot commit.
CREATE TABLE finance_direct_swap_commands (
    buyer_user_id BIGINT NOT NULL REFERENCES reader_wallet_accounts(user_id)
        ON DELETE RESTRICT,
    request_key TEXT NOT NULL CHECK (char_length(request_key)=43),
    request_digest BYTEA NOT NULL CHECK (octet_length(request_digest)=32),
    source_purchase_id BIGINT NOT NULL,
    target_chapter_id BIGINT NOT NULL,
    target_version_id BIGINT NOT NULL,
    source_amount_minor BIGINT NOT NULL CHECK (source_amount_minor>0),
    target_amount_minor BIGINT NOT NULL CHECK (target_amount_minor>0),
    new_purchase_id BIGINT UNIQUE REFERENCES chapter_purchase_facts(id)
        ON DELETE RESTRICT,
    balance_after_minor BIGINT CHECK (balance_after_minor>=0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (buyer_user_id,request_key),
    FOREIGN KEY (source_purchase_id,buyer_user_id)
        REFERENCES chapter_purchase_facts(id,user_id) ON DELETE RESTRICT,
    FOREIGN KEY (target_chapter_id,target_version_id)
        REFERENCES chapter_versions(chapter_id,id) ON DELETE RESTRICT,
    CHECK ((new_purchase_id IS NULL)=(balance_after_minor IS NULL))
);

CREATE FUNCTION d06_guard_swap_command()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' THEN
        RAISE EXCEPTION 'swap command is immutable' USING ERRCODE='23514';
    END IF;
    IF TG_OP='INSERT' THEN
        IF NEW.new_purchase_id IS NOT NULL THEN
            RAISE EXCEPTION 'swap claim must start uncompleted' USING ERRCODE='23514';
        END IF;
        RETURN NEW;
    END IF;
    IF ROW(NEW.buyer_user_id,NEW.request_key,NEW.request_digest,
           NEW.source_purchase_id,NEW.target_chapter_id,NEW.target_version_id,
           NEW.source_amount_minor,NEW.target_amount_minor,NEW.created_at)
       IS DISTINCT FROM
       ROW(OLD.buyer_user_id,OLD.request_key,OLD.request_digest,
           OLD.source_purchase_id,OLD.target_chapter_id,OLD.target_version_id,
           OLD.source_amount_minor,OLD.target_amount_minor,OLD.created_at)
       OR OLD.new_purchase_id IS NOT NULL
       OR NEW.new_purchase_id IS NULL OR NEW.balance_after_minor IS NULL
    THEN
        RAISE EXCEPTION 'swap command identity/receipt cannot change'
            USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER d06_swap_command_guard
    BEFORE INSERT OR UPDATE OR DELETE ON finance_direct_swap_commands
    FOR EACH ROW EXECUTE FUNCTION d06_guard_swap_command();

-- This posting is the authoritative link between the original source and
-- the new sale. Creator reversals are separate immutable, exact lines.
CREATE TABLE finance_direct_swap_postings (
    source_purchase_id BIGINT PRIMARY KEY
        REFERENCES finance_chapter_postings(purchase_id) ON DELETE RESTRICT,
    new_purchase_id BIGINT NOT NULL UNIQUE
        REFERENCES finance_chapter_postings(purchase_id) ON DELETE RESTRICT,
    buyer_user_id BIGINT NOT NULL,
    request_key TEXT NOT NULL,
    source_amount_minor BIGINT NOT NULL CHECK (source_amount_minor>0),
    target_amount_minor BIGINT NOT NULL CHECK (target_amount_minor>0),
    platform_reversal_minor BIGINT NOT NULL CHECK (platform_reversal_minor>=0),
    creator_reversal_minor BIGINT NOT NULL CHECK (creator_reversal_minor>=0),
    reversal_reference TEXT NOT NULL UNIQUE
        CHECK (char_length(reversal_reference) BETWEEN 8 AND 160),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    FOREIGN KEY (buyer_user_id,request_key)
        REFERENCES finance_direct_swap_commands(buyer_user_id,request_key)
        ON DELETE RESTRICT,
    CHECK (source_purchase_id<>new_purchase_id),
    CHECK (source_amount_minor=platform_reversal_minor+creator_reversal_minor)
);
CREATE TABLE finance_direct_swap_creator_reversals (
    source_purchase_id BIGINT NOT NULL
        REFERENCES finance_direct_swap_postings(source_purchase_id) ON DELETE RESTRICT,
    user_id BIGINT NOT NULL,
    amount_minor BIGINT NOT NULL CHECK (amount_minor>0),
    PRIMARY KEY (source_purchase_id,user_id),
    FOREIGN KEY(source_purchase_id,user_id)
        REFERENCES finance_creator_earnings(purchase_id,user_id) ON DELETE RESTRICT
);
CREATE TRIGGER d06_swap_postings_immutable
    BEFORE UPDATE OR DELETE ON finance_direct_swap_postings
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();
CREATE TRIGGER d06_swap_creator_reversals_immutable
    BEFORE UPDATE OR DELETE ON finance_direct_swap_creator_reversals
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();

-- Deferred finance check: neither a detached swap header nor a fabricated
-- wallet credit can commit. The target sale also has D-08's own constraints.
CREATE FUNCTION d06_verify_atomic_swap()
RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    mismatched_lines BIGINT;
    original_credit_count BIGINT;
    source_credit_balance BIGINT;
    target_debit_balance BIGINT;
    target_debit_count BIGINT;
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM chapter_purchase_facts oldp
        JOIN chapters source_ch ON source_ch.id=oldp.chapter_id
        JOIN books source_book ON source_book.id=source_ch.book_id
        JOIN finance_chapter_postings oldf ON oldf.purchase_id=oldp.id
        JOIN finance_platform_credits oldpc ON oldpc.purchase_id=oldp.id
        JOIN chapter_purchase_facts newp ON newp.id=NEW.new_purchase_id
        JOIN chapters target_ch ON target_ch.id=newp.chapter_id
        JOIN books target_book ON target_book.id=target_ch.book_id
        JOIN chapter_publications publication ON publication.chapter_id=target_ch.id
            AND publication.version_id=newp.version_id
        JOIN finance_chapter_postings newf ON newf.purchase_id=newp.id
        JOIN finance_platform_credits newpc ON newpc.purchase_id=newp.id
        JOIN finance_direct_swap_commands cmd
          ON cmd.buyer_user_id=NEW.buyer_user_id AND cmd.request_key=NEW.request_key
        JOIN finance_direct_refund_policy_catalog source_policy
          ON source_policy.version=oldp.refund_policy_version
        JOIN finance_direct_refund_policy_catalog target_policy
          ON target_policy.version=newp.refund_policy_version
        JOIN library_purchased_chapters source_library
          ON source_library.purchase_id=oldp.id
             AND source_library.chapter_id=oldp.chapter_id
             AND source_library.user_id=oldp.user_id
        JOIN library_purchased_chapters target_library
          ON target_library.purchase_id=newp.id
             AND target_library.chapter_id=newp.chapter_id
             AND target_library.user_id=newp.user_id
             AND target_library.active_version_id=newp.version_id
        JOIN chapter_purchase_revocations revoke
          ON revoke.purchase_id=oldp.id
            AND revoke.reason='purchase_swap'
            AND revoke.reversal_reference=NEW.reversal_reference
        WHERE oldp.id=NEW.source_purchase_id
          AND oldp.user_id=NEW.buyer_user_id
          AND newp.user_id=NEW.buyer_user_id
          AND oldp.currency='RUB' AND newp.currency='RUB'
          AND oldp.amount_minor=NEW.source_amount_minor
          AND newp.amount_minor=NEW.target_amount_minor
          AND cmd.source_purchase_id=oldp.id
          AND cmd.new_purchase_id=newp.id
          AND cmd.target_chapter_id=newp.chapter_id
          AND cmd.target_version_id=newp.version_id
          AND cmd.source_amount_minor=NEW.source_amount_minor
          AND cmd.target_amount_minor=NEW.target_amount_minor
          AND source_ch.book_id<>target_ch.book_id
          AND source_book.owner_user_id IS NOT NULL
          AND source_book.owner_user_id=target_book.owner_user_id
          AND target_book.owner_user_id<>NEW.buyer_user_id
          AND target_ch.access_level='paid'
          AND target_ch.visibility='public'
          AND target_ch.price_amount=NEW.target_amount_minor
          AND target_ch.price_amount>0 AND NOT target_ch.admin_hold
          AND NOT target_ch.is_deleted
          AND (target_ch.reader_released_at IS NULL
               OR target_ch.reader_released_at<=NEW.created_at)
          AND target_book.visibility='public'
          AND NOT target_book.admin_hold AND NOT target_book.is_deleted
          AND oldp.first_grant_tracking_version='c06-first-issue-v1'
          AND newp.first_grant_tracking_version='c06-first-issue-v1'
          AND oldf.refund_policy_version=oldp.refund_policy_version
          AND oldf.refund_deadline_at=oldp.refund_deadline_at
          AND newf.refund_policy_version=newp.refund_policy_version
          AND newf.refund_deadline_at=newp.refund_deadline_at
          AND source_policy.approved_at<=oldp.acquired_at
          AND source_policy.effective_at<=oldp.acquired_at
          AND target_policy.approved_at<=newp.acquired_at
          AND target_policy.effective_at<=newp.acquired_at
          AND oldp.refund_deadline_at>=NEW.created_at
          AND oldp.refund_deadline_at<=oldp.acquired_at
                 +(source_policy.window_seconds*INTERVAL '1 second')
          AND newp.refund_deadline_at<=newp.acquired_at
                 +(target_policy.window_seconds*INTERVAL '1 second')
          AND oldf.gross_minor=NEW.source_amount_minor
          AND oldf.platform_fee_minor=NEW.platform_reversal_minor
          AND oldf.creator_pool_minor=NEW.creator_reversal_minor
          AND oldpc.amount_minor=NEW.platform_reversal_minor
          AND newf.gross_minor=NEW.target_amount_minor
          AND newpc.amount_minor=newf.platform_fee_minor
          AND NOT EXISTS (
             SELECT 1 FROM chapter_purchase_first_exposures opened
             WHERE opened.purchase_id=oldp.id
          )
          AND NOT EXISTS (
             SELECT 1 FROM finance_direct_refund_requests pending
             WHERE pending.purchase_id=oldp.id
               AND pending.state IN ('pending','processing')
          )
          AND NOT EXISTS (
             SELECT 1 FROM finance_direct_refund_postings refund
             WHERE refund.purchase_id=oldp.id
          )
          AND (SELECT COUNT(*) FROM reader_wallet_entries debit
               WHERE debit.purchase_id=oldp.id AND debit.user_id=oldp.user_id
                 AND debit.kind='chapter_purchase'
                 AND debit.delta_minor=-oldp.amount_minor
                 AND debit.reference=oldp.settlement_reference)=1
    ) THEN
        RAISE EXCEPTION 'swap purchase, policy, target or revocation is invalid'
            USING ERRCODE='23514';
    END IF;

    SELECT COUNT(*),MAX(e.balance_after_minor)
      INTO original_credit_count,source_credit_balance
    FROM reader_wallet_entries e
    WHERE e.purchase_id=NEW.source_purchase_id
      AND e.user_id=NEW.buyer_user_id
      AND e.kind='chapter_swap_credit'
      AND e.delta_minor=NEW.source_amount_minor
      AND e.reference=NEW.reversal_reference;
    SELECT COUNT(*),MAX(e.balance_after_minor)
      INTO target_debit_count,target_debit_balance
    FROM reader_wallet_entries e
    JOIN chapter_purchase_facts p ON p.id=e.purchase_id
    WHERE e.purchase_id=NEW.new_purchase_id
      AND e.user_id=NEW.buyer_user_id
      AND e.kind='chapter_purchase'
      AND e.delta_minor=-NEW.target_amount_minor
      AND e.reference=p.settlement_reference;
    IF original_credit_count<>1 OR target_debit_count<>1
       OR source_credit_balance IS DISTINCT FROM
          target_debit_balance+NEW.target_amount_minor
       OR NOT EXISTS (
          SELECT 1 FROM reader_wallet_accounts w
          WHERE w.user_id=NEW.buyer_user_id AND w.currency='RUB'
            AND w.balance_minor=target_debit_balance
       )
    THEN
        RAISE EXCEPTION 'swap wallet conservation failed'
            USING ERRCODE='23514';
    END IF;

    SELECT COUNT(*) INTO mismatched_lines
    FROM (SELECT user_id,amount_minor FROM finance_creator_earnings
          WHERE purchase_id=NEW.source_purchase_id) original
    FULL OUTER JOIN
         (SELECT user_id,amount_minor FROM finance_direct_swap_creator_reversals
          WHERE source_purchase_id=NEW.source_purchase_id) reversed
    ON original.user_id=reversed.user_id
    WHERE original.user_id IS NULL OR reversed.user_id IS NULL
       OR original.amount_minor IS DISTINCT FROM reversed.amount_minor;
    IF mismatched_lines<>0 OR
       (SELECT COALESCE(SUM(amount_minor),0)
        FROM finance_direct_swap_creator_reversals
        WHERE source_purchase_id=NEW.source_purchase_id)<>NEW.creator_reversal_minor
    THEN
        RAISE EXCEPTION 'swap creator reversals are incomplete'
            USING ERRCODE='23514';
    END IF;
    RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER d06_swap_posting_complete
    AFTER INSERT ON finance_direct_swap_postings
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION d06_verify_atomic_swap();

CREATE FUNCTION d06_verify_swap_command_completed()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM finance_direct_swap_commands cmd
        JOIN finance_direct_swap_postings posting
          ON posting.source_purchase_id=cmd.source_purchase_id
         AND posting.new_purchase_id=cmd.new_purchase_id
         AND posting.buyer_user_id=cmd.buyer_user_id
         AND posting.request_key=cmd.request_key
        JOIN reader_wallet_accounts w ON w.user_id=cmd.buyer_user_id
        WHERE cmd.buyer_user_id=NEW.buyer_user_id AND cmd.request_key=NEW.request_key
          AND cmd.new_purchase_id IS NOT NULL
          AND cmd.balance_after_minor=w.balance_minor
    ) THEN
        RAISE EXCEPTION 'unfinished swap command cannot commit'
            USING ERRCODE='23514';
    END IF;
    RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER d06_swap_command_complete
    AFTER INSERT ON finance_direct_swap_commands
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION d06_verify_swap_command_completed();

CREATE FUNCTION d06_verify_swap_credit_has_posting()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM finance_direct_swap_postings p
        WHERE p.source_purchase_id=NEW.purchase_id
          AND p.buyer_user_id=NEW.user_id
          AND p.source_amount_minor=NEW.delta_minor
          AND p.reversal_reference=NEW.reference
    ) THEN
        RAISE EXCEPTION 'orphan swap buyer credit' USING ERRCODE='23514';
    END IF;
    RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER d06_swap_credit_complete
    AFTER INSERT ON reader_wallet_entries
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW WHEN (NEW.kind='chapter_swap_credit')
    EXECUTE FUNCTION d06_verify_swap_credit_has_posting();

CREATE FUNCTION d06_verify_swap_revocation_has_posting()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM finance_direct_swap_postings p
        WHERE p.source_purchase_id=NEW.purchase_id
          AND p.reversal_reference=NEW.reversal_reference
    ) THEN
        RAISE EXCEPTION 'orphan swap revocation' USING ERRCODE='23514';
    END IF;
    RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER d06_swap_revocation_complete
    AFTER INSERT ON chapter_purchase_revocations
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW WHEN (NEW.reason='purchase_swap')
    EXECUTE FUNCTION d06_verify_swap_revocation_has_posting();
