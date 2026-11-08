-- S030 / B-09: irreversible ownership transfer audit. Shares are referenced
-- by revision only; changing an owner never rewrites purchase snapshots.
CREATE TABLE book_owner_transfer_events (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE RESTRICT,
    previous_owner_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    new_owner_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    share_revision BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK(previous_owner_user_id<>new_owner_user_id),
    FOREIGN KEY(book_id,share_revision)
        REFERENCES revenue_share_versions(book_id,revision) ON DELETE RESTRICT
);
CREATE INDEX s030_owner_events_by_book ON book_owner_transfer_events(book_id,id DESC);
CREATE TRIGGER s030_owner_events_immutable
    BEFORE UPDATE OR DELETE ON book_owner_transfer_events
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();
