-- D-05 / B-09: close the raw DELETE gap in Layer membership governance.
-- Additive migration only: preserve all previously applied SQLx checksums.
-- The existing b09_layer_member_fence handles INSERT / UPDATE but not DELETE.
-- Deny deleting an active membership while this Book is in the current
-- Layer composition or carries an outstanding, non-revoked paid Layer term.
-- Inactive membership cleanup remains permitted.

CREATE FUNCTION d05_block_layer_member_delete()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT OLD.is_active THEN
        RETURN OLD;
    END IF;

    -- Serialize deletion with D-05 composition changes and paid-term issuance
    -- using the same canonical Book lock as the INSERT / UPDATE fence.
    PERFORM 1 FROM books WHERE id=OLD.book_id FOR UPDATE;

    IF EXISTS (
        SELECT 1 FROM commerce_layers l
        JOIN commerce_layer_version_books vb
          ON vb.layer_version_id=l.current_version_id
        WHERE vb.book_id=OLD.book_id
    ) OR EXISTS (
        -- Match b09_layer_block_ownership_change: a removed Book still has
        -- outstanding lifetime or future prepaid Layer delivery obligations.
        SELECT 1 FROM commerce_layer_terms t
        JOIN commerce_layer_versions origin
          ON origin.id=t.access_origin_version_id AND origin.layer_id=t.layer_id
        JOIN commerce_layer_versions v
          ON v.layer_id=t.layer_id AND v.version_no>=origin.version_no
        JOIN commerce_layer_version_books vb
          ON vb.layer_version_id=v.id AND vb.book_id=OLD.book_id
        WHERE (t.expires_at IS NULL OR t.expires_at>now())
          AND (t.expires_at IS NULL OR v.created_at<t.expires_at)
          AND NOT EXISTS (
              SELECT 1 FROM commerce_layer_term_revocations r
              WHERE r.term_id=t.id
          )
    ) THEN
        RAISE EXCEPTION 'book membership deletion locked by Layer'
            USING ERRCODE='23514';
    END IF;
    RETURN OLD;
END;
$$;

CREATE TRIGGER d05_layer_member_delete_fence
    BEFORE DELETE ON book_members
    FOR EACH ROW EXECUTE FUNCTION d05_block_layer_member_delete();
