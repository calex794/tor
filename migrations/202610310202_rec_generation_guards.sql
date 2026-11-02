-- Validated scoring edges cannot be rewritten. B deletion/retention may remove edges.
CREATE FUNCTION rec_generation_writable() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF NOT EXISTS(SELECT 1 FROM rec_generations WHERE id=NEW.generation_id AND status='building') THEN
    RAISE EXCEPTION 'recommendation generation is immutable' USING ERRCODE='23514';
  END IF;
  RETURN NEW;
END;
$$;
CREATE TRIGGER rec_edge_write_guard BEFORE INSERT OR UPDATE ON rec_candidate_edges
  FOR EACH ROW EXECUTE FUNCTION rec_generation_writable();
CREATE INDEX rec_quality_retention ON rec_quality_daily(bucket_date);
