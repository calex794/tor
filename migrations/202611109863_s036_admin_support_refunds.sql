-- S036: administrator-initiated support exceptions are a separate audited
-- basis for individual internal-wallet purchase refunds. Consumer policy
-- and the first-exposure guard remain mandatory for ordinary refunds.
-- No external provider payment or credit is claimed by this internal posting.
-- PostgreSQL requires an explicit unique key on every referenced column
-- of a composite FK, even when 'id' is independently unique.
ALTER TABLE finance_direct_refund_requests
    ADD CONSTRAINT d06_admin_support_request_triple_key UNIQUE(id,purchase_id,user_id);

CREATE TABLE finance_support_refund_intents (
    request_id BIGINT PRIMARY KEY,
    purchase_id BIGINT NOT NULL,
    buyer_user_id BIGINT NOT NULL,
    admin_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    support_conversation_id BIGINT NOT NULL
        REFERENCES support_conversations(id) ON DELETE RESTRICT,
    basis TEXT NOT NULL DEFAULT 'support_discretion'
        CHECK (basis = 'support_discretion'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    FOREIGN KEY (request_id,purchase_id,buyer_user_id)
        REFERENCES finance_direct_refund_requests(id,purchase_id,user_id)
        ON DELETE RESTRICT
);
CREATE FUNCTION d06_validate_support_refund_intent()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM support_conversations c
        WHERE c.id=NEW.support_conversation_id
          AND c.requester_user_id=NEW.buyer_user_id
          AND EXISTS (
              SELECT 1 FROM support_messages m
              WHERE m.conversation_id=c.id AND m.sender_kind='user'
          )
    ) OR NOT EXISTS (
        SELECT 1 FROM account_roles a
        JOIN users u ON u.id=a.user_id
        WHERE a.user_id=NEW.admin_user_id
          AND a.role='administrator' AND a.is_active
          AND u.account_state='active'
    ) THEN
        RAISE EXCEPTION 'trusted support case and administrator required'
          USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER d06_validate_support_refund_intent
    BEFORE INSERT ON finance_support_refund_intents
    FOR EACH ROW EXECUTE FUNCTION d06_validate_support_refund_intent();
CREATE TRIGGER d06_support_refund_intents_immutable
    BEFORE UPDATE OR DELETE ON finance_support_refund_intents
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();

-- Search history is audited without storing the typed name or message text.
ALTER TABLE finance_direct_refund_admin_access_audit
    DROP CONSTRAINT finance_direct_refund_admin_access_audit_action_check;
ALTER TABLE finance_direct_refund_admin_access_audit
    ADD CONSTRAINT finance_direct_refund_admin_access_audit_action_check
    CHECK (action IN ('list_refunds','search_users','view_purchases'));

-- Preserve normal consumer settlement eligibility. Only a separately
-- recorded admin/support intent may use the discretionary return basis.
CREATE OR REPLACE FUNCTION d06_verify_atomic_refund()
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
        LEFT JOIN finance_direct_refund_policy_catalog policy
          ON policy.version=p.refund_policy_version
        LEFT JOIN finance_support_refund_intents support
          ON support.request_id=q.id AND support.purchase_id=p.id
         AND support.buyer_user_id=p.user_id
        WHERE p.id=NEW.purchase_id AND p.user_id=NEW.buyer_user_id
          AND p.amount_minor=NEW.amount_minor
          AND p.currency='RUB'
          AND fp.currency='RUB'
          AND fp.gross_minor=NEW.amount_minor
          AND fp.platform_fee_minor=NEW.platform_reversal_minor
          AND fp.creator_pool_minor=NEW.creator_reversal_minor
          AND pc.amount_minor=NEW.platform_reversal_minor
          AND fp.refund_policy_version IS NOT DISTINCT FROM p.refund_policy_version
          AND fp.refund_deadline_at IS NOT DISTINCT FROM p.refund_deadline_at
          AND q.state='approved'
          AND q.decided_at IS NOT NULL
          AND (
            (
              support.request_id IS NOT NULL
              AND support.admin_user_id=q.decision_actor_id
              AND EXISTS (
                SELECT 1 FROM support_conversations convo
                WHERE convo.id=support.support_conversation_id
                  AND convo.requester_user_id=p.user_id
              )
            )
            OR (
              support.request_id IS NULL
              AND p.first_grant_tracking_version='c06-first-issue-v1'
              AND policy.approved_at<=p.acquired_at
              AND policy.effective_at<=p.acquired_at
              AND p.refund_deadline_at <= p.acquired_at
                  + (policy.window_seconds * INTERVAL '1 second')
              AND q.decided_at<=p.refund_deadline_at
              AND NOT EXISTS (
                SELECT 1 FROM chapter_purchase_first_exposures exposure
                WHERE exposure.purchase_id=p.id
              )
            )
          )
          AND q.decision_actor_id IS NOT NULL
          AND q.decision_reference=NEW.reversal_reference

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
