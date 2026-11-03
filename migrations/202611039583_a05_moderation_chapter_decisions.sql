-- A-05/MOD-C02: distinguish NEwBOO hide vs block while retaining
-- the independent TOR author publication/version/price contract.
ALTER TABLE chapters
    ADD COLUMN moderation_state TEXT NOT NULL DEFAULT 'none';

-- Preserve existing moderator-held chapters as generic holds.
UPDATE chapters SET moderation_state='hold' WHERE admin_hold;

ALTER TABLE chapters
    ADD CONSTRAINT chapters_moderation_state_allowed
    CHECK (moderation_state IN ('none','hold','hidden','blocked'));
-- Prevent hidden/blocked states from being served with admin_hold=false.
ALTER TABLE chapters
    ADD CONSTRAINT chapters_moderation_state_hold_consistent
    CHECK ((moderation_state = 'none') = (NOT admin_hold));

-- Older fixture/worker SQL may update only admin_hold. Convert those
-- generic hold toggles to the corresponding state, but reject any attempt
-- to remove a specifically hidden/blocked moderator decision that way.
CREATE FUNCTION chapters_sync_legacy_moderation_hold()
RETURNS trigger LANGUAGE plpgsql AS $
BEGIN
    IF TG_OP = 'INSERT' THEN
        IF NEW.admin_hold AND NEW.moderation_state = 'none' THEN
            NEW.moderation_state := 'hold';
        END IF;
    ELSIF NEW.admin_hold IS DISTINCT FROM OLD.admin_hold
          AND NEW.moderation_state IS NOT DISTINCT FROM OLD.moderation_state THEN
        IF OLD.moderation_state IN ('hidden','blocked') THEN
            RAISE EXCEPTION 'cannot bypass specific chapter moderation decision'
                USING ERRCODE='23514';
        END IF;
        NEW.moderation_state := CASE WHEN NEW.admin_hold THEN 'hold' ELSE 'none' END;
    END IF;
    RETURN NEW;
END;
$;
CREATE TRIGGER chapters_sync_legacy_moderation_hold
    BEFORE INSERT OR UPDATE OF admin_hold ON chapters
    FOR EACH ROW EXECUTE FUNCTION chapters_sync_legacy_moderation_hold();

CREATE TABLE moderation_chapter_decision_events (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    actor_user_id BIGINT NOT NULL CHECK (actor_user_id > 0),
    book_id BIGINT NOT NULL CHECK (book_id > 0),
    chapter_id BIGINT NOT NULL CHECK (chapter_id > 0),
    before_state TEXT NOT NULL,
    after_state TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    CONSTRAINT moderation_chapter_decision_changed CHECK (before_state <> after_state),
    CONSTRAINT moderation_chapter_decision_before_allowed
        CHECK (before_state IN ('none','hold','hidden','blocked')),
    CONSTRAINT moderation_chapter_decision_after_allowed
        CHECK (after_state IN ('none','hold','hidden','blocked'))
);
CREATE INDEX moderation_chapter_decision_events_chapter_recent
    ON moderation_chapter_decision_events (chapter_id, id DESC);

CREATE FUNCTION moderation_chapter_decision_immutable()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'moderation chapter decision event is immutable'
        USING ERRCODE='23514';
END;
$$;
CREATE TRIGGER moderation_chapter_decision_events_immutable
    BEFORE UPDATE OR DELETE ON moderation_chapter_decision_events
    FOR EACH ROW EXECUTE FUNCTION moderation_chapter_decision_immutable();
