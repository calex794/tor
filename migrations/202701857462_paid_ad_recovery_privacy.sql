CREATE TABLE finance_ad_publisher_transfers (
 id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
 user_id BIGINT NOT NULL REFERENCES users(id),
 command_key TEXT NOT NULL CHECK(char_length(command_key)=43),
 batch_limit INTEGER NOT NULL CHECK(batch_limit BETWEEN 1 AND 100),
 status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN('pending','posted')),
 amount_minor BIGINT NOT NULL DEFAULT 0 CHECK(amount_minor>=0),
 UNIQUE(user_id,command_key)
);
ALTER TABLE finance_ad_publisher_credits ADD COLUMN publisher_transfer_id BIGINT REFERENCES finance_ad_publisher_transfers(id),
 ADD CHECK((status='credited')=(publisher_transfer_id IS NOT NULL));
CREATE INDEX finance_ad_transfer_credits ON finance_ad_publisher_credits(publisher_transfer_id) WHERE publisher_transfer_id IS NOT NULL;
CREATE FUNCTION finance_ad_transfer_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF ROW(OLD.id,OLD.user_id,OLD.command_key,OLD.batch_limit) IS DISTINCT FROM ROW(NEW.id,NEW.user_id,NEW.command_key,NEW.batch_limit)
  OR OLD.status='posted' OR NEW.status<>'posted' THEN
  RAISE EXCEPTION 'immutable publisher transfer intent' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END; $$;
CREATE TRIGGER finance_ad_transfer_immutable BEFORE UPDATE ON finance_ad_publisher_transfers
 FOR EACH ROW EXECUTE FUNCTION finance_ad_transfer_guard();
CREATE TRIGGER finance_ad_transfer_no_delete BEFORE DELETE ON finance_ad_publisher_transfers
 FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();
CREATE FUNCTION finance_ad_transfer_proof() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE transfer finance_ad_publisher_transfers; n BIGINT; amount NUMERIC; wrong BOOLEAN;
BEGIN
 IF TG_TABLE_NAME='finance_ad_publisher_credits' THEN
  IF NEW.publisher_transfer_id IS NULL THEN RETURN NULL; END IF;
  SELECT * INTO transfer FROM finance_ad_publisher_transfers WHERE id=NEW.publisher_transfer_id;
 ELSE SELECT * INTO transfer FROM finance_ad_publisher_transfers WHERE id=NEW.id; END IF;
 IF transfer.status<>'posted' THEN RETURN NULL; END IF;
 SELECT count(*),COALESCE(sum(amount_minor),0),COALESCE(bool_or(publisher_id<>transfer.user_id OR status<>'credited'),false)
 INTO n,amount,wrong FROM finance_ad_publisher_credits WHERE publisher_transfer_id=transfer.id;
 IF n>transfer.batch_limit OR amount<>transfer.amount_minor OR wrong THEN
  RAISE EXCEPTION 'publisher transfer lacks exact credit evidence' USING ERRCODE='23514'; END IF;
 RETURN NULL;
END; $$;
CREATE CONSTRAINT TRIGGER finance_ad_transfer_receipt AFTER INSERT OR UPDATE ON finance_ad_publisher_transfers
 DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION finance_ad_transfer_proof();
CREATE CONSTRAINT TRIGGER finance_ad_credit_transfer AFTER INSERT OR UPDATE ON finance_ad_publisher_credits
 DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION finance_ad_transfer_proof();

CREATE TABLE ad_planner_control(singleton BOOLEAN PRIMARY KEY CHECK(singleton),dirty BOOLEAN NOT NULL);
-- Financial evidence survives; fraud identities do not become reading history.
ALTER TABLE ad_viewability_opportunities ALTER COLUMN viewer_id DROP NOT NULL,ALTER COLUMN session_hash DROP NOT NULL;
DROP TRIGGER ad_opportunity_immutable ON ad_viewability_opportunities;
CREATE FUNCTION ad_opportunity_retention() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' THEN
  IF OLD.expires_at>clock_timestamp()-interval '1 day' THEN RAISE EXCEPTION 'live ad proof' USING ERRCODE='23514'; END IF;
  RETURN OLD;
 END IF;
 IF (to_jsonb(OLD)-'viewer_id'-'session_hash') IS DISTINCT FROM (to_jsonb(NEW)-'viewer_id'-'session_hash')
  OR OLD.expires_at>clock_timestamp()-interval '1 day' OR NEW.viewer_id IS NOT NULL OR NEW.session_hash IS NOT NULL THEN
  RAISE EXCEPTION 'immutable live ad proof' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END; $$;
CREATE TRIGGER ad_opportunity_privacy BEFORE UPDATE OR DELETE ON ad_viewability_opportunities
 FOR EACH ROW EXECUTE FUNCTION ad_opportunity_retention();
