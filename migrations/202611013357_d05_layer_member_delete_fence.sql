-- D-05 / B-09: raw DELETE and historical owner UPDATE membership fence.
-- New additive SQLx migration; never rewrite earlier applied migrations.
-- Current Layer composition freezes all active member mutations. Historical
-- paid terms preserve the canonical active owner, not former collaborators.

-- Reuse the exact outstanding obligation rule of b09_layer_block_ownership_change.
-- Future prepaid renewals count even if their start date has not arrived.
CREATE FUNCTION d05_has_outstanding_book_layer_term(p_book_id BIGINT)
RETURNS BOOLEAN LANGUAGE sql STABLE AS $$
    SELECT EXISTS (
        SELECT 1 FROM commerce_layer_terms t
        JOIN commerce_layer_versions origin
          ON origin.id=t.access_origin_version_id AND origin.layer_id=t.layer_id
        JOIN commerce_layer_versions v
          ON v.layer_id=t.layer_id AND v.version_no>=origin.version_no
        JOIN commerce_layer_version_books vb
          ON vb.layer_version_id=v.id AND vb.book_id=p_book_id
        WHERE (t.expires_at IS NULL OR t.expires_at>now())
          AND (t.expires_at IS NULL OR v.created_at<t.expires_at)
          AND NOT EXISTS (
              SELECT 1 FROM commerce_layer_term_revocations r
              WHERE r.term_id=t.id
          )
    );
$$;

CREATE FUNCTION d05_block_layer_member_delete()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    -- Historical inactive collaborators are safe to prune.
    IF NOT OLD.is_active THEN
        RETURN OLD;
    END IF;

    -- Same canonical Book lock as Layer composition, checkout and B-09.
    PERFORM 1 FROM books WHERE id=OLD.book_id FOR UPDATE;
    IF EXISTS (
        SELECT 1 FROM commerce_layers l
        JOIN commerce_layer_version_books vb
          ON vb.layer_version_id=l.current_version_id
        WHERE vb.book_id=OLD.book_id
    ) OR (
        OLD.role='owner'
        AND d05_has_outstanding_book_layer_term(OLD.book_id)
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

-- After a Book leaves the *current* Layer, the pre-existing B-09
-- INSERT/UPDATE fence no longer sees that composition. An active paid term
-- still forbids downgrading or deactivating the canonical owner membership.
-- Otherwise raw UPDATE would remain an alternative to a denied DELETE.
CREATE FUNCTION d05_block_historical_layer_owner_update()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT OLD.is_active OR OLD.role<>'owner'
       OR (NEW.is_active AND NEW.role='owner') THEN
        RETURN NEW;
    END IF;

    PERFORM 1 FROM books WHERE id=OLD.book_id FOR UPDATE;
    IF d05_has_outstanding_book_layer_term(OLD.book_id) THEN
        RAISE EXCEPTION 'book owner membership update locked by paid Layer'
            USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER d05_historical_layer_owner_update_fence
    BEFORE UPDATE OF role,is_active ON book_members
    FOR EACH ROW EXECUTE FUNCTION d05_block_historical_layer_owner_update();
