-- Retired assignments are read/delete-only. SET NULL would fire their statement
-- write fence even for a new generation with no assignments. RESTRICT keeps any
-- surviving historical reference intact and allows deletion of unreferenced models.
ALTER TABLE rec_assignments DROP CONSTRAINT rec_assignments_generation_id_fkey;
ALTER TABLE rec_assignments ADD CONSTRAINT rec_assignments_generation_id_fkey
  FOREIGN KEY(generation_id) REFERENCES rec_generations(id) ON DELETE RESTRICT;

-- A new metadata/manual model cannot inherit or relabel a behavioural generation.
-- Existing accepted migration bytes remain unchanged. This transition has no user rows.
ALTER TABLE rec_generations ADD COLUMN model_version text NOT NULL DEFAULT 'legacy_feedback_v1'
  CHECK(model_version IN ('legacy_feedback_v1','metadata_manual_v1'));
UPDATE rec_generations SET status='rejected',finished_at=COALESCE(finished_at,now())
  WHERE model_version='legacy_feedback_v1' AND status='building';
UPDATE rec_runtime_policy SET active_generation=NULL,previous_generation=NULL,
  rollout_percent=0,hold=true,revision=revision+1
  WHERE active_generation IS NOT NULL OR previous_generation IS NOT NULL
     OR rollout_percent<>0 OR NOT hold;

CREATE FUNCTION rec_contextual_generation_guard() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog,public AS $$
BEGIN
  IF TG_OP='INSERT' THEN
    IF NEW.model_version<>'metadata_manual_v1' OR NEW.status<>'building'
       OR NEW.cursor_book_id<>0 OR NEW.finished_at IS NOT NULL OR NEW.manifest_hash IS NOT NULL THEN
      RAISE EXCEPTION 'explicit contextual building generation required' USING ERRCODE='23514';
    END IF;
  ELSE
    IF OLD.model_version<>'metadata_manual_v1' OR OLD.status<>'building'
       OR NEW.model_version<>OLD.model_version OR NEW.id<>OLD.id
       OR NEW.created_at<>OLD.created_at OR NEW.source_window_end<>OLD.source_window_end
       OR NEW.cursor_book_id<OLD.cursor_book_id THEN
      RAISE EXCEPTION 'generation model and completed generation are immutable' USING ERRCODE='23514';
    END IF;
    IF (NEW.status='building' AND (NEW.finished_at IS NOT NULL OR NEW.manifest_hash IS NOT NULL))
       OR (NEW.status='validated' AND (NEW.finished_at IS NULL OR NEW.manifest_hash IS NULL))
       OR (NEW.status='rejected' AND NEW.finished_at IS NULL) THEN
      RAISE EXCEPTION 'invalid contextual generation transition' USING ERRCODE='23514';
    END IF;
  END IF;
  RETURN NEW;
END;
$$;
CREATE TRIGGER rec_contextual_generation_guard BEFORE INSERT OR UPDATE ON rec_generations
  FOR EACH ROW EXECUTE FUNCTION rec_contextual_generation_guard();

-- Lock the parent until commit: validation cannot overtake an edge write.
CREATE OR REPLACE FUNCTION rec_generation_writable() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog,public AS $$
DECLARE model text; state text;
BEGIN
  SELECT model_version,status INTO model,state FROM public.rec_generations
    WHERE id=NEW.generation_id FOR SHARE;
  IF model IS DISTINCT FROM 'metadata_manual_v1' OR state IS DISTINCT FROM 'building'
     OR NEW.seed_id<=0 OR NEW.seed_id=NEW.book_id
     OR NOT((NEW.source='content' AND NEW.reason='similar_by_content')
         OR (NEW.source='related_books' AND NEW.reason='manually_related')) THEN
    RAISE EXCEPTION 'only building contextual metadata/manual edges are writable' USING ERRCODE='23514';
  END IF;
  IF TG_OP='UPDATE' AND OLD.generation_id<>NEW.generation_id THEN
    RAISE EXCEPTION 'generation edge cannot be moved' USING ERRCODE='23514';
  END IF;
  RETURN NEW;
END;
$$;
CREATE FUNCTION rec_contextual_policy_guard() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog,public AS $$
DECLARE candidate uuid;
BEGIN
  IF NEW.rollout_percent NOT IN (0,100) THEN
    RAISE EXCEPTION 'contextual model has no identity-based rollout' USING ERRCODE='23514';
  END IF;
  FOREACH candidate IN ARRAY ARRAY[NEW.active_generation,NEW.previous_generation] LOOP
    IF candidate IS NOT NULL AND NOT EXISTS(
      SELECT 1 FROM public.rec_generations WHERE id=candidate
      AND model_version='metadata_manual_v1' AND status='validated'
      AND finished_at IS NOT NULL AND octet_length(manifest_hash)=32
    ) THEN
      RAISE EXCEPTION 'only validated contextual generations can be selected' USING ERRCODE='23514';
    END IF;
  END LOOP;
  RETURN NEW;
END;
$$;
CREATE TRIGGER rec_contextual_policy_guard BEFORE INSERT OR UPDATE ON rec_runtime_policy
  FOR EACH ROW EXECUTE FUNCTION rec_contextual_policy_guard();
