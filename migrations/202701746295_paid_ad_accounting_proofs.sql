-- Deferred, indexed proofs close the counter and cross-owner posting gaps.
CREATE INDEX ad_last_settled ON ad_impression_reservations(intent_id,ordinal DESC) WHERE settled_at IS NOT NULL;
CREATE FUNCTION finance_ad_verify_counters() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE f finance_ad_funding; reserved BIGINT; settled BIGINT; c_status TEXT;
BEGIN
 SELECT * INTO f FROM finance_ad_funding WHERE intent_id=NEW.intent_id;
 IF f.product='chapter' THEN
  SELECT COALESCE(max(ordinal),0) INTO reserved FROM ad_impression_reservations WHERE intent_id=f.intent_id;
  SELECT COALESCE(max(ordinal),0) INTO settled FROM ad_impression_reservations WHERE intent_id=f.intent_id AND settled_at IS NOT NULL;
  SELECT status INTO c_status FROM ad_chapter_campaigns WHERE id=f.campaign_id;
 ELSE
  SELECT COALESCE(max(ordinal),0) INTO reserved FROM finance_ad_daily_units WHERE intent_id=f.intent_id;
  settled:=reserved;
  SELECT status INTO c_status FROM ad_legacy_campaigns WHERE id=f.campaign_id;
 END IF;
 IF f.reserved_units<>reserved OR f.settled_minor::NUMERIC<>floor(f.budget_minor::NUMERIC*settled/f.target_units)
   OR f.publisher_settled_minor::NUMERIC<>floor(f.settled_minor::NUMERIC*f.publisher_share_bps/10000)
   OR (f.status='cancelled') IS DISTINCT FROM (c_status='cancelled')
   OR (f.status='cancelled' AND NOT EXISTS(SELECT 1 FROM finance_ad_cancellations WHERE intent_id=f.intent_id))
 THEN RAISE EXCEPTION 'advertising counters lack durable evidence' USING ERRCODE='23514'; END IF;
 RETURN NULL;
END; $$;
CREATE CONSTRAINT TRIGGER finance_ad_counter_proof AFTER INSERT OR UPDATE ON finance_ad_funding
 DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION finance_ad_verify_counters();

CREATE FUNCTION finance_ad_verify_effect() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE t finance_ad_transactions; f finance_ad_funding; matched BOOLEAN; cnt BIGINT;
        pending_owner BIGINT; available_owner BIGINT; wallet_owner BIGINT;
BEGIN
 IF TG_TABLE_NAME='finance_ad_entries' THEN
  SELECT * INTO t FROM finance_ad_transactions WHERE id=NEW.transaction_id;
 ELSE t:=NEW; END IF;
 SELECT * INTO f FROM finance_ad_funding WHERE intent_id=t.intent_id;
 SELECT count(*),max(owner_user_id) FILTER(WHERE account='publisher_pending'),
  max(owner_user_id) FILTER(WHERE account='publisher_available'),max(owner_user_id) FILTER(WHERE account='wallet')
 INTO cnt,pending_owner,available_owner,wallet_owner FROM finance_ad_entries WHERE transaction_id=t.id;
 matched:=true;
 IF t.operation IN('reserve','settle') THEN
  IF f.product='chapter' THEN
   SELECT EXISTS(SELECT 1 FROM ad_impression_reservations r WHERE r.intent_id=f.intent_id
     AND (CASE t.operation WHEN 'reserve' THEN r.reservation_transaction_id ELSE r.settlement_transaction_id END)=t.id
     AND r.gross_minor=t.amount_minor AND (t.operation<>'settle' OR
       ((r.publisher_minor=0 OR pending_owner=r.publisher_id)
        AND cnt=(CASE WHEN r.gross_minor>0 THEN 1 ELSE 0 END)
          +(CASE WHEN r.publisher_minor>0 THEN 1 ELSE 0 END)+(CASE WHEN r.platform_minor>0 THEN 1 ELSE 0 END)
        AND (r.ordinal=1 OR EXISTS(SELECT 1 FROM ad_impression_reservations previous
          WHERE previous.intent_id=r.intent_id AND previous.ordinal=r.ordinal-1 AND previous.settled_at IS NOT NULL))))) INTO matched;
  ELSE
   SELECT EXISTS(SELECT 1 FROM finance_ad_daily_units d JOIN finance_ad_intents i ON i.id=d.intent_id
     WHERE d.intent_id=f.intent_id AND d.ordinal<=f.reserved_units AND t.amount_minor=i.rate_minor
       AND (CASE t.operation WHEN 'reserve' THEN d.reservation_transaction_id ELSE d.settlement_transaction_id END)=t.id
       AND (d.ordinal=1 OR EXISTS(SELECT 1 FROM finance_ad_daily_units previous
          WHERE previous.intent_id=d.intent_id AND previous.ordinal=d.ordinal-1))) INTO matched;
  END IF;
 ELSIF t.operation='publisher_mature' THEN
  SELECT EXISTS(SELECT 1 FROM finance_ad_publisher_credits c WHERE c.maturity_transaction_id=t.id
    AND c.intent_id=t.intent_id AND c.amount_minor=t.amount_minor AND c.publisher_id=pending_owner
    AND c.publisher_id=available_owner AND c.clear_at<=clock_timestamp()
    AND t.identity='ads:mature:'||c.id) INTO matched;
 ELSIF t.operation='publisher_credit' THEN
  SELECT EXISTS(SELECT 1 FROM finance_ad_publisher_credits c WHERE c.wallet_entry_id=t.wallet_entry_id
    AND c.intent_id=t.intent_id AND c.amount_minor=t.amount_minor AND c.publisher_id=available_owner
    AND c.publisher_id=wallet_owner AND c.status='credited' AND c.clear_at<=clock_timestamp()
    AND t.identity='ads:publisher:'||c.id) INTO matched;
 ELSIF t.operation='refund' THEN
  SELECT EXISTS(SELECT 1 FROM finance_ad_cancellations c WHERE c.transaction_id=t.id
    AND c.intent_id=t.intent_id AND c.refunded_minor=t.amount_minor) INTO matched;
 END IF;
 IF NOT matched THEN RAISE EXCEPTION 'advertising posting has no exact operation evidence' USING ERRCODE='23514'; END IF;
 RETURN NULL;
END; $$;
CREATE CONSTRAINT TRIGGER finance_ad_effect_proof AFTER INSERT ON finance_ad_transactions
 DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION finance_ad_verify_effect();
CREATE CONSTRAINT TRIGGER finance_ad_effect_entry_proof AFTER INSERT ON finance_ad_entries
 DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION finance_ad_verify_effect();

CREATE FUNCTION finance_ad_verify_credit() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE c finance_ad_publisher_credits;
BEGIN
 SELECT * INTO c FROM finance_ad_publisher_credits WHERE id=NEW.id;
 IF NOT EXISTS(SELECT 1 FROM ad_impression_reservations r WHERE r.settlement_transaction_id=c.settlement_transaction_id
    AND r.intent_id=c.intent_id AND r.publisher_id=c.publisher_id AND r.publisher_minor=c.amount_minor
    AND c.clear_at>=r.settled_at+(SELECT hold_seconds FROM finance_ad_intents WHERE id=c.intent_id)*interval '1 second')
   OR (c.status IN('available','credited') AND NOT EXISTS(SELECT 1 FROM finance_ad_transactions t
     WHERE t.id=c.maturity_transaction_id AND t.intent_id=c.intent_id AND t.operation='publisher_mature'
       AND t.amount_minor=c.amount_minor AND t.identity='ads:mature:'||c.id))
   OR (c.status='credited' AND NOT EXISTS(SELECT 1 FROM finance_ad_transactions t
     WHERE t.wallet_entry_id=c.wallet_entry_id AND t.intent_id=c.intent_id AND t.operation='publisher_credit'
       AND t.amount_minor=c.amount_minor AND t.identity='ads:publisher:'||c.id))
 THEN RAISE EXCEPTION 'publisher credit lacks exact settlement/maturity/wallet proof' USING ERRCODE='23514'; END IF;
 RETURN NULL;
END; $$;
CREATE CONSTRAINT TRIGGER finance_ad_credit_proof AFTER INSERT OR UPDATE ON finance_ad_publisher_credits
 DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION finance_ad_verify_credit();
