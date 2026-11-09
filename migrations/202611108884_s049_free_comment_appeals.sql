-- S049: independent author appeal of a sanctioned free-edition E comment.
-- The author and the immutable hide event authorize filing independently of chapter visibility.
-- Reviewer must be an active *different* site administrator. Filing or restoration never grants access to a restricted or removed chapter.
CREATE TABLE social_free_comment_appeals (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    hide_event_id BIGINT NOT NULL UNIQUE
        REFERENCES social_free_comment_review_events(id) ON DELETE RESTRICT,
    author_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    reason TEXT NOT NULL CHECK (char_length(reason) BETWEEN 20 AND 1000),
    command_key TEXT NOT NULL CHECK (command_key ~ '^[A-Za-z0-9_-]{43}$'),
    state TEXT NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending','denied','restored')),
    filed_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    UNIQUE (author_user_id,command_key)
);
CREATE INDEX social_free_comment_appeals_pending
    ON social_free_comment_appeals(filed_at,id) WHERE state='pending';
CREATE INDEX social_free_comment_appeals_author
    ON social_free_comment_appeals(author_user_id,filed_at DESC);

CREATE TABLE social_free_comment_appeal_decisions (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    appeal_id BIGINT NOT NULL UNIQUE
        REFERENCES social_free_comment_appeals(id) ON DELETE RESTRICT,
    reviewer_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    decision TEXT NOT NULL CHECK (decision IN ('denied','restored')),
    note TEXT NOT NULL CHECK (char_length(note) BETWEEN 1 AND 500),
    decided_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);

CREATE FUNCTION s049_free_appeal_decision_immutable() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'S049 free appeal decision is immutable' USING ERRCODE='23514';
END; $$;
CREATE TRIGGER s049_free_appeal_decision_immutable_guard
    BEFORE UPDATE OR DELETE ON social_free_comment_appeal_decisions
    FOR EACH ROW EXECUTE FUNCTION s049_free_appeal_decision_immutable();

CREATE FUNCTION s049_free_appeal_evidence_immutable() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF ROW(OLD.hide_event_id,OLD.author_user_id,OLD.reason,OLD.command_key,OLD.filed_at)
       IS DISTINCT FROM
       ROW(NEW.hide_event_id,NEW.author_user_id,NEW.reason,NEW.command_key,NEW.filed_at)
    THEN
        RAISE EXCEPTION 'S049 free appeal submission evidence is immutable'
            USING ERRCODE='23514';
    END IF;
    IF OLD.state <> 'pending' OR NEW.state NOT IN ('denied','restored') THEN
        RAISE EXCEPTION 'S049 free appeal outcome is final' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END; $$;
CREATE TRIGGER s049_free_appeal_evidence_immutable_guard
    BEFORE UPDATE ON social_free_comment_appeals
    FOR EACH ROW EXECUTE FUNCTION s049_free_appeal_evidence_immutable();
