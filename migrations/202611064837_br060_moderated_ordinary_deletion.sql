-- BR-060: ordinary deletion requires a separate site moderation decision.
-- The two immutable facts are retained independently of Book hard/legal erasure.
-- A request references the exact lifecycle revision produced by owner "start".
CREATE TABLE book_ordinary_deletion_requests (
    book_id BIGINT NOT NULL CHECK (book_id > 0),
    request_revision BIGINT NOT NULL CHECK (request_revision > 1),
    owner_user_id BIGINT NOT NULL CHECK (owner_user_id > 0),
    previous_lifecycle_status TEXT NOT NULL CHECK (
        previous_lifecycle_status IN ('in_progress','completed','abandoned','on_hiatus')
    ),
    previous_visibility TEXT NOT NULL,
    requested_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (book_id, request_revision)
);

CREATE TABLE book_ordinary_deletion_decisions (
    book_id BIGINT NOT NULL,
    request_revision BIGINT NOT NULL,
    moderator_user_id BIGINT NOT NULL CHECK (moderator_user_id > 0),
    decision TEXT NOT NULL CHECK (decision IN ('approve','reject')),
    decided_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (book_id, request_revision),
    CONSTRAINT book_ordinary_deletion_decisions_request_fk FOREIGN KEY (book_id,request_revision)
      REFERENCES book_ordinary_deletion_requests(book_id,request_revision) ON DELETE RESTRICT
);
CREATE INDEX book_ordinary_deletion_decisions_moderator_recent
    ON book_ordinary_deletion_decisions(moderator_user_id,decided_at DESC);

CREATE FUNCTION book_ordinary_deletion_review_immutable()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'ordinary deletion request and decision history is append-only'
      USING ERRCODE='23514';
END;
$$;
CREATE TRIGGER book_ordinary_deletion_requests_immutable
    BEFORE UPDATE OR DELETE ON book_ordinary_deletion_requests
    FOR EACH ROW EXECUTE FUNCTION book_ordinary_deletion_review_immutable();
CREATE TRIGGER book_ordinary_deletion_decisions_immutable
    BEFORE UPDATE OR DELETE ON book_ordinary_deletion_decisions
    FOR EACH ROW EXECUTE FUNCTION book_ordinary_deletion_review_immutable();

-- Deliberately no FK to books/users: legal erasure must not cascade away
-- immutable moderation evidence. Application checks ownership and live roles
-- under PostgreSQL locks. No automatic backfill for legacy pending requests:
-- an unapproved legacy request remains fail-closed.
