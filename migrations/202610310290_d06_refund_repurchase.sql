-- COM-05 / BR-038: trusted repurchase after a fully settled consumer refund.
-- Original purchase, wallet debit, platform/creator credits, refund and their
-- reversals stay immutable. A NEW purchase has a NEW settlement and grant.
-- Only the canonical purchase INSERT trigger may switch the library pointer.

-- Reacquisition of the SAME paid V1 requires a new immutable purchase fact.
-- Keep the unique identity (id,user_id,chapter_id,version_id) used by FKs.
ALTER TABLE chapter_purchase_facts
    DROP CONSTRAINT chapter_purchase_facts_user_id_chapter_id_version_id_key;

CREATE TABLE library_purchase_reacquisitions (
    new_purchase_id BIGINT PRIMARY KEY
        REFERENCES chapter_purchase_facts(id) ON DELETE RESTRICT,
    old_purchase_id BIGINT NOT NULL UNIQUE
        REFERENCES chapter_purchase_facts(id) ON DELETE RESTRICT,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    chapter_id BIGINT NOT NULL REFERENCES chapters(id) ON DELETE RESTRICT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    CHECK (old_purchase_id <> new_purchase_id)
);

CREATE TRIGGER b05_reacquisition_audit_immutable
    BEFORE UPDATE OR DELETE ON library_purchase_reacquisitions
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();

-- Protect the one live library pointer against direct SQL mutation.
-- pg_trigger_depth()=2 here proves a nested update from the purchase INSERT
-- trigger. The application DB role cannot create its own triggers or DDL.
CREATE FUNCTION b05_guard_library_reacquisition()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'library pointer deletion is forbidden'
          USING ERRCODE='23514';
    END IF;
    IF pg_trigger_depth() <> 2
       OR NEW.user_id IS DISTINCT FROM OLD.user_id
       OR NEW.chapter_id IS DISTINCT FROM OLD.chapter_id
       OR NEW.purchase_id IS NOT DISTINCT FROM OLD.purchase_id
       OR NEW.created_at < OLD.created_at
       OR NOT EXISTS (
           SELECT 1
           FROM finance_direct_refund_postings refund
           JOIN chapter_purchase_revocations revoked
             ON revoked.purchase_id=refund.purchase_id
            AND revoked.reason='refund'
            AND revoked.reversal_reference=refund.reversal_reference
           JOIN chapter_purchase_facts newer ON newer.id=NEW.purchase_id
           WHERE refund.purchase_id=OLD.purchase_id
             AND newer.user_id=OLD.user_id
             AND newer.chapter_id=OLD.chapter_id
             AND newer.version_id=NEW.active_version_id
             AND newer.acquired_at>=refund.created_at
             AND NOT EXISTS (
                 SELECT 1 FROM chapter_purchase_revocations r
                 WHERE r.purchase_id=newer.id
             )
       ) THEN
        RAISE EXCEPTION 'library pointer is not a trusted refunded repurchase'
          USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;

DROP TRIGGER purchased_library_no_mutation ON library_purchased_chapters;
CREATE TRIGGER b05_library_reacquisition_only
    BEFORE UPDATE OR DELETE ON library_purchased_chapters
    FOR EACH ROW EXECUTE FUNCTION b05_guard_library_reacquisition();

CREATE FUNCTION b05_guard_reacquisition_audit_insert()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF pg_trigger_depth() <> 2 OR NOT EXISTS (
        SELECT 1 FROM library_purchased_chapters l
        JOIN chapter_purchase_facts newer ON newer.id=NEW.new_purchase_id
        WHERE l.user_id=NEW.user_id AND l.chapter_id=NEW.chapter_id
          AND l.purchase_id=NEW.new_purchase_id
          AND newer.user_id=NEW.user_id AND newer.chapter_id=NEW.chapter_id
    ) THEN
        RAISE EXCEPTION 'reacquisition audit must be born in purchase trigger'
          USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER b05_reacquisition_audit_writer
    BEFORE INSERT ON library_purchase_reacquisitions
    FOR EACH ROW EXECUTE FUNCTION b05_guard_reacquisition_audit_insert();

-- This single handler is the ONLY trusted transition. Calling checkout with
-- a different idempotency key creates a new chapter_purchase_facts row. The
-- old library pin is changed ONLY when its exact refund is fully posted.
-- Do not accept chargeback, fraud, a pending request or a forged revocation.
CREATE OR REPLACE FUNCTION install_purchased_chapter()
RETURNS trigger LANGUAGE plpgsql
SECURITY DEFINER SET search_path=pg_catalog,public
AS $purchase_grant$
DECLARE
    old_id BIGINT;
BEGIN
    SELECT l.purchase_id INTO old_id
    FROM public.library_purchased_chapters l
    WHERE l.user_id=NEW.user_id AND l.chapter_id=NEW.chapter_id
    FOR UPDATE;

    IF old_id IS NULL THEN
        INSERT INTO public.library_purchased_chapters
            (user_id,chapter_id,active_version_id,purchase_id)
        VALUES(NEW.user_id,NEW.chapter_id,NEW.version_id,NEW.id);
        RETURN NEW;
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM public.finance_direct_refund_postings refund
        JOIN public.chapter_purchase_revocations revoked
          ON revoked.purchase_id=refund.purchase_id
         AND revoked.reason='refund'
         AND revoked.reversal_reference=refund.reversal_reference
        WHERE refund.purchase_id=old_id
          AND refund.buyer_user_id=NEW.user_id
          AND NEW.acquired_at>=refund.created_at
    ) THEN
        RAISE EXCEPTION 'already owned; no settled consumer refund'
          USING ERRCODE='23505';
    END IF;

    UPDATE public.library_purchased_chapters
    SET active_version_id=NEW.version_id,
        purchase_id=NEW.id,
        created_at=clock_timestamp()
    WHERE user_id=NEW.user_id AND chapter_id=NEW.chapter_id
      AND purchase_id=old_id;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'concurrent library repurchase conflict'
          USING ERRCODE='23514';
    END IF;

    INSERT INTO public.library_purchase_reacquisitions
        (old_purchase_id,new_purchase_id,user_id,chapter_id)
    VALUES(old_id,NEW.id,NEW.user_id,NEW.chapter_id);
    RETURN NEW;
END;
$purchase_grant$;

-- Stop app-role manual rewrites. The SECURITY DEFINER purchase trigger retains
-- narrow, verified UPDATE authority for the pointer change.
REVOKE UPDATE, DELETE ON public.library_purchased_chapters FROM tor_app;

-- An append-only reacquisition must have its own buyer debit and matching
-- new D-08 allocation. The prior paid refund/reversal is never reused as
-- funding, and a checkout with creator accounting disabled cannot commit.
CREATE FUNCTION b05_verify_reacquisition_finance()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM chapter_purchase_facts newp
        JOIN chapter_purchase_facts oldp
          ON oldp.id=NEW.old_purchase_id
        JOIN finance_direct_refund_postings refund
          ON refund.purchase_id=oldp.id
        JOIN chapter_purchase_revocations revoke
          ON revoke.purchase_id=oldp.id
         AND revoke.reason='refund'
         AND revoke.reversal_reference=refund.reversal_reference
        JOIN library_purchased_chapters l
          ON l.user_id=newp.user_id AND l.chapter_id=newp.chapter_id
         AND l.purchase_id=newp.id AND l.active_version_id=newp.version_id
        JOIN finance_chapter_postings newfinance
          ON newfinance.purchase_id=newp.id
        JOIN finance_platform_credits platform
          ON platform.purchase_id=newp.id
        WHERE newp.id=NEW.new_purchase_id
          AND newp.user_id=NEW.user_id AND newp.chapter_id=NEW.chapter_id
          AND oldp.user_id=newp.user_id AND oldp.chapter_id=newp.chapter_id
          AND newp.acquired_at>=refund.created_at
          AND newfinance.gross_minor=newp.amount_minor
          AND platform.amount_minor=newfinance.platform_fee_minor
          AND (SELECT COUNT(*) FROM reader_wallet_entries debit
               WHERE debit.purchase_id=newp.id AND debit.user_id=newp.user_id
                 AND debit.kind='chapter_purchase'
                 AND debit.delta_minor=-newp.amount_minor
                 AND debit.reference=newp.settlement_reference)=1
          AND NOT EXISTS (
              SELECT 1 FROM chapter_purchase_revocations r
              WHERE r.purchase_id=newp.id
          )
    ) THEN
        RAISE EXCEPTION 'reacquisition lacks a settled original refund or new money'
          USING ERRCODE='23514';
    END IF;
    RETURN NULL;
END;
$$;

CREATE CONSTRAINT TRIGGER b05_reacquisition_finance_complete
    AFTER INSERT ON library_purchase_reacquisitions
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION b05_verify_reacquisition_finance();
