-- B-09a / S030: creator-project ACL is independent of Team and purchases.
-- books.owner_user_id remains the single authoritative owner until the
-- B-09 transfer boundary (tier eligibility, Layer obligations, CAS) lands.
CREATE TABLE book_members (
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE RESTRICT,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    role TEXT NOT NULL CHECK (role IN ('owner','translator','moderator')),
    is_active BOOLEAN NOT NULL DEFAULT TRUE,
    share_percent SMALLINT NOT NULL DEFAULT 0
        CHECK (share_percent BETWEEN 0 AND 100),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (book_id, user_id)
);
CREATE UNIQUE INDEX book_members_one_active_owner
    ON book_members(book_id)
    WHERE is_active AND role='owner';
CREATE INDEX book_members_active_actor
    ON book_members(user_id, book_id)
    WHERE is_active;

-- Preserve every pre-existing owner, including draft/private books.
-- Imported catalog rows with NULL owner do not acquire any actor rights.
INSERT INTO book_members (book_id,user_id,role)
SELECT id,owner_user_id,'owner'
FROM books
WHERE owner_user_id IS NOT NULL;

-- All existing book creation paths, including legacy SQL fixtures, acquire
-- the owner row atomically with the new Book. This trigger does not authorize
-- ownership transfer: no UPDATE trigger is installed. Owner changes must use
-- the future B-09 transfer transaction and update both records.
CREATE FUNCTION b09_seed_book_owner()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO book_members(book_id,user_id,role)
    VALUES (NEW.id,NEW.owner_user_id,'owner');
    RETURN NEW;
END; $$;
CREATE TRIGGER b09_seed_book_owner_after_insert
    AFTER INSERT ON books
    FOR EACH ROW WHEN (NEW.owner_user_id IS NOT NULL)
    EXECUTE FUNCTION b09_seed_book_owner();
