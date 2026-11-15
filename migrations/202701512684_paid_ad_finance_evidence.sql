-- Every posted purchase has the exact F campaign, immutable intent, canonical
-- wallet debit and balanced D subledger. Raw links are never sufficient proof.
CREATE FUNCTION finance_ad_verify_funding() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE f finance_ad_funding; i finance_ad_intents; matched BOOLEAN;
BEGIN
    IF TG_TABLE_NAME='finance_ad_intents' THEN
        IF NEW.status<>'posted' THEN RETURN NULL; END IF;
        SELECT * INTO f FROM finance_ad_funding WHERE intent_id=NEW.id;
    ELSE
        f:=NEW;
    END IF;
    SELECT * INTO i FROM finance_ad_intents WHERE id=f.intent_id;
    IF f.intent_id IS NULL OR i.id IS NULL OR i.status<>'posted'
       OR ROW(i.advertiser_id,i.product,i.campaign_id,i.price_minor,i.target_units,i.publisher_share_bps)
          IS DISTINCT FROM ROW(f.advertiser_id,f.product,f.campaign_id,f.budget_minor,f.target_units,f.publisher_share_bps)
       OR NOT EXISTS(SELECT 1 FROM reader_wallet_entries e
           JOIN finance_ad_transactions t ON t.wallet_entry_id=e.id
           WHERE e.id=f.wallet_entry_id AND e.user_id=f.advertiser_id
             AND e.kind='ad_funding_debit' AND e.delta_minor=-f.budget_minor
             AND e.purchase_id IS NULL AND e.layer_term_id IS NULL
             AND e.reference='ads:fund:'||f.intent_id
             AND t.identity=e.reference AND t.intent_id=f.intent_id
             AND t.operation='fund' AND t.amount_minor=f.budget_minor)
    THEN RAISE EXCEPTION 'ad funding lacks exact wallet evidence' USING ERRCODE='23514'; END IF;
    IF f.product='daily' THEN
        SELECT EXISTS(SELECT 1 FROM ad_legacy_campaigns c
            WHERE c.id=f.campaign_id AND c.advertiser_id=f.advertiser_id
              AND c.funding_intent_id=f.intent_id AND c.book_id=i.book_id
              AND c.offer_id=i.offer_id AND c.offer_revision=i.offer_revision
              AND c.starts_at=i.starts_at AND c.ends_at=i.ends_at
              AND c.total_days=i.total_days AND c.price_minor=i.price_minor
              AND c.rate_minor=i.rate_minor AND c.slot=i.slot) INTO matched;
    ELSE
        SELECT EXISTS(SELECT 1 FROM ad_chapter_campaigns c
            WHERE c.id=f.campaign_id AND c.advertiser_id=f.advertiser_id
              AND c.funding_intent_id=f.intent_id AND c.promoted_book_id=i.book_id
              AND c.package_id=i.offer_id AND c.package_revision=i.offer_revision) INTO matched;
    END IF;
    IF NOT matched THEN RAISE EXCEPTION 'ad funding lacks exact campaign' USING ERRCODE='23514'; END IF;
    RETURN NULL;
END; $$;
CREATE CONSTRAINT TRIGGER finance_ad_funding_evidence AFTER INSERT ON finance_ad_funding
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION finance_ad_verify_funding();
CREATE CONSTRAINT TRIGGER finance_ad_intent_evidence AFTER INSERT OR UPDATE ON finance_ad_intents
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION finance_ad_verify_funding();

CREATE FUNCTION finance_ad_verify_transaction() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE total NUMERIC; rows_count BIGINT; wallet_delta BIGINT; wallet_owner BIGINT;
        escrow_delta BIGINT; pending_delta BIGINT; available_delta BIGINT;
        reserved_delta BIGINT; platform_delta BIGINT; f finance_ad_funding;
        posting finance_ad_transactions;
BEGIN
    IF TG_TABLE_NAME='finance_ad_entries' THEN
        SELECT * INTO posting FROM finance_ad_transactions WHERE id=NEW.transaction_id;
    ELSE
        posting:=NEW;
    END IF;
    SELECT COALESCE(sum(delta_minor),0),count(*),
       COALESCE(sum(delta_minor) FILTER(WHERE account='wallet'),0),
       max(owner_user_id) FILTER(WHERE account='wallet'),
       COALESCE(sum(delta_minor) FILTER(WHERE account='escrow'),0),
       COALESCE(sum(delta_minor) FILTER(WHERE account='publisher_pending'),0),
       COALESCE(sum(delta_minor) FILTER(WHERE account='publisher_available'),0),
       COALESCE(sum(delta_minor) FILTER(WHERE account='reserved'),0),
       COALESCE(sum(delta_minor) FILTER(WHERE account='platform'),0)
    INTO total,rows_count,wallet_delta,wallet_owner,escrow_delta,pending_delta,available_delta,reserved_delta,platform_delta
    FROM finance_ad_entries WHERE transaction_id=posting.id;
    SELECT * INTO f FROM finance_ad_funding WHERE intent_id=posting.intent_id;
    IF total<>0 OR (posting.amount_minor>0 AND rows_count<2)
       OR (posting.amount_minor=0 AND rows_count<>0)
       OR (posting.operation='fund' AND (wallet_delta<>-posting.amount_minor OR escrow_delta<>posting.amount_minor
           OR wallet_owner<>f.advertiser_id OR rows_count<>2))
       OR (posting.operation='reserve' AND (escrow_delta<>-posting.amount_minor OR reserved_delta<>posting.amount_minor
           OR (posting.amount_minor>0 AND rows_count<>2)))
       OR (posting.operation='settle' AND (reserved_delta<>-posting.amount_minor OR pending_delta+platform_delta<>posting.amount_minor
           OR pending_delta<0 OR platform_delta<0 OR wallet_delta<>0 OR escrow_delta<>0 OR available_delta<>0))
       OR (posting.operation='refund' AND (escrow_delta<>-posting.amount_minor OR wallet_delta<>posting.amount_minor
           OR (posting.amount_minor>0 AND (wallet_owner<>f.advertiser_id OR rows_count<>2))))
       OR (posting.operation='publisher_mature' AND (pending_delta<>-posting.amount_minor OR available_delta<>posting.amount_minor OR rows_count<>2))
       OR (posting.operation='publisher_credit' AND (available_delta<>-posting.amount_minor OR wallet_delta<>posting.amount_minor OR rows_count<>2))
    THEN RAISE EXCEPTION 'unbalanced or wrong advertising posting' USING ERRCODE='23514'; END IF;
    IF posting.wallet_entry_id IS NOT NULL THEN
        IF NOT EXISTS(SELECT 1 FROM reader_wallet_entries e
             WHERE e.id=posting.wallet_entry_id AND e.reference=posting.identity
             AND e.user_id=wallet_owner AND e.delta_minor=wallet_delta
             AND e.kind=CASE posting.operation WHEN 'fund' THEN 'ad_funding_debit'
                 WHEN 'refund' THEN 'ad_refund_credit' WHEN 'publisher_credit' THEN 'ad_publisher_credit' ELSE '' END
             AND e.purchase_id IS NULL AND e.layer_term_id IS NULL)
        THEN RAISE EXCEPTION 'ad wallet posting has wrong evidence' USING ERRCODE='23514'; END IF;
    ELSIF wallet_delta<>0 THEN
        RAISE EXCEPTION 'ad wallet effect has no canonical journal' USING ERRCODE='23514';
    END IF;
    RETURN NULL;
END; $$;
CREATE CONSTRAINT TRIGGER finance_ad_balanced_transaction AFTER INSERT ON finance_ad_transactions
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION finance_ad_verify_transaction();
CREATE CONSTRAINT TRIGGER finance_ad_balanced_entry AFTER INSERT ON finance_ad_entries
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION finance_ad_verify_transaction();

CREATE FUNCTION finance_ad_verify_campaign() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE product_name TEXT;
BEGIN
    product_name:=CASE TG_TABLE_NAME WHEN 'ad_legacy_campaigns' THEN 'daily' ELSE 'chapter' END;
    IF NOT EXISTS(SELECT 1 FROM finance_ad_funding f JOIN finance_ad_intents i ON i.id=f.intent_id
        WHERE f.intent_id=NEW.funding_intent_id AND f.campaign_id=NEW.id AND f.product=product_name
          AND f.advertiser_id=NEW.advertiser_id AND i.status='posted')
    THEN RAISE EXCEPTION 'campaign lacks posted funding' USING ERRCODE='23514'; END IF;
    RETURN NULL;
END; $$;
CREATE CONSTRAINT TRIGGER ad_daily_funded_evidence AFTER INSERT ON ad_legacy_campaigns
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION finance_ad_verify_campaign();
CREATE CONSTRAINT TRIGGER ad_chapter_funded_evidence AFTER INSERT ON ad_chapter_campaigns
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION finance_ad_verify_campaign();

CREATE FUNCTION finance_ad_verify_wallet_entry() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS(SELECT 1 FROM finance_ad_transactions t
       JOIN finance_ad_entries e ON e.transaction_id=t.id AND e.account='wallet'
       WHERE t.wallet_entry_id=NEW.id AND t.identity=NEW.reference
         AND e.owner_user_id=NEW.user_id AND e.delta_minor=NEW.delta_minor)
    THEN RAISE EXCEPTION 'orphan advertising wallet entry' USING ERRCODE='23514'; END IF;
    RETURN NULL;
END; $$;
CREATE CONSTRAINT TRIGGER finance_ad_wallet_evidence AFTER INSERT ON reader_wallet_entries
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW
    WHEN(NEW.kind IN('ad_funding_debit','ad_refund_credit','ad_publisher_credit'))
    EXECUTE FUNCTION finance_ad_verify_wallet_entry();

CREATE FUNCTION finance_ad_verify_cancellation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS(SELECT 1 FROM finance_ad_funding f
        JOIN finance_ad_transactions t ON t.id=NEW.transaction_id
        WHERE f.intent_id=NEW.intent_id AND f.status='cancelled'
          AND f.refunded_minor=NEW.refunded_minor AND f.settled_minor+f.refunded_minor=f.budget_minor
          AND t.intent_id=f.intent_id AND t.operation='refund' AND t.amount_minor=NEW.refunded_minor
          AND t.identity='ads:refund:'||f.intent_id)
    THEN RAISE EXCEPTION 'cancellation lacks exact unused refund' USING ERRCODE='23514'; END IF;
    RETURN NULL;
END; $$;
CREATE CONSTRAINT TRIGGER finance_ad_refund_evidence AFTER INSERT ON finance_ad_cancellations
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION finance_ad_verify_cancellation();
