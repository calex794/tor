-- I-reviewed FLOW-03 collection evidence. A worker's process flag cannot prove
-- whether a canonical C command on another instance opted into FLOW capture.
-- This is a bounded permission receipt, not a copy of the C event payload.
CREATE TABLE reader_flow_read_receipts (
    event_id uuid PRIMARY KEY REFERENCES outbox_events(id) ON DELETE CASCADE
        DEFERRABLE INITIALLY DEFERRED,
    subject_id uuid NOT NULL,
    flow_epoch uuid NOT NULL,
    occurred_at timestamptz NOT NULL,
    FOREIGN KEY(subject_id,flow_epoch) REFERENCES rec_subjects(subject_id,flow_epoch)
        ON DELETE CASCADE
);
CREATE INDEX reader_flow_reads_scope
    ON reader_flow_read_receipts(subject_id,flow_epoch);
CREATE INDEX reader_flow_reads_expiry
    ON reader_flow_read_receipts(occurred_at,event_id);

CREATE VIEW reader_flow_verified_read_receipts WITH (security_barrier=true) AS
SELECT event_id,subject_id,flow_epoch,occurred_at FROM reader_flow_read_receipts;

-- C writes its canonical row before the matching outbox row in the same
-- statement. The deferred event FK is checked at commit, when both exist.
-- Only the real canonical INSERT can capture: replay cannot backfill a fact
-- first accepted while FLOW was off or before its explicit consent epoch.
CREATE FUNCTION reader_flow_capture_read() RETURNS trigger
LANGUAGE plpgsql
SET lock_timeout='25ms'
AS $$
DECLARE
    current_scope record;
    captured_at timestamptz;
BEGIN
    IF current_setting('tor.flow_read_capture_enabled',true) IS DISTINCT FROM '1' THEN
        RETURN NULL;
    END IF;
    IF TG_TABLE_NAME='reader_chapter_starts' THEN
        captured_at := NEW.started_at;
    ELSIF TG_TABLE_NAME='reader_chapter_completions' THEN
        captured_at := NEW.completed_at;
    ELSE
        RETURN NULL;
    END IF;
    BEGIN
        SELECT subject_id,flow_epoch INTO current_scope
        FROM reader_flow_consent
        WHERE user_id=NEW.user_id AND flow_enabled_at<=captured_at
        FOR UPDATE;
        IF NOT FOUND THEN
            RETURN NULL;
        END IF;
        INSERT INTO reader_flow_read_receipts
            (event_id,subject_id,flow_epoch,occurred_at)
        VALUES(NEW.source_event_id,current_scope.subject_id,
               current_scope.flow_epoch,captured_at);
    EXCEPTION WHEN OTHERS THEN
        -- Lock/storage failure affects this optional capture only. Explicit
        -- statement cancellation is not caught by PL/pgSQL WHEN OTHERS.
        PERFORM set_config('tor.flow_read_capture_failed','1',true);
    END;
    RETURN NULL;
END
$$;
CREATE TRIGGER reader_flow_capture_chapter_start
    AFTER INSERT ON reader_chapter_starts
    FOR EACH ROW EXECUTE FUNCTION reader_flow_capture_read();
CREATE TRIGGER reader_flow_capture_chapter_completion
    AFTER INSERT ON reader_chapter_completions
    FOR EACH ROW EXECUTE FUNCTION reader_flow_capture_read();

-- The additive I hook shares the subject transaction with both existing FLOW
-- erasure hooks. No scope FK may outlive an epoch rotation or account deletion.
CREATE FUNCTION reader_flow_erase_read_scope() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' THEN
        DELETE FROM reader_flow_read_receipts
        WHERE subject_id=OLD.subject_id AND flow_epoch=OLD.flow_epoch;
        RETURN OLD;
    END IF;
    IF OLD.subject_id IS DISTINCT FROM NEW.subject_id
        OR OLD.flow_epoch IS DISTINCT FROM NEW.flow_epoch THEN
        DELETE FROM reader_flow_read_receipts
        WHERE subject_id=OLD.subject_id AND flow_epoch=OLD.flow_epoch;
    END IF;
    RETURN NEW;
END
$$;
CREATE TRIGGER reader_flow_erase_replaced_read_scope
    BEFORE UPDATE OF subject_id,flow_epoch ON rec_subjects
    FOR EACH ROW EXECUTE FUNCTION reader_flow_erase_read_scope();
CREATE TRIGGER reader_flow_erase_deleted_read_scope
    BEFORE DELETE ON rec_subjects
    FOR EACH ROW EXECUTE FUNCTION reader_flow_erase_read_scope();

COMMENT ON TABLE reader_flow_read_receipts IS
    'C atomic FLOW collection proof for one canonical event and current consent scope; no payload copy; raw30d; off commands and replay never backfill.';
COMMENT ON VIEW reader_flow_verified_read_receipts IS
    'C-approved F read port; join exact event and scope to historical canonical evidence. Worker health cannot substitute for per-command capture.';
