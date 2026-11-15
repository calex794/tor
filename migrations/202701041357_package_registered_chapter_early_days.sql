-- H/UI-21: creator-selected per-chapter early window from chapter registration.
-- Legacy H/UI-19 book-wide windows remain until their owners choose a new mode.
-- No production data is touched until this migration is explicitly deployed.
ALTER TABLE chapters ADD COLUMN registered_at TIMESTAMPTZ;
-- A default added after the column prevents assigning "now" to historic rows.
ALTER TABLE chapters ALTER COLUMN registered_at SET DEFAULT now();
-- Historic chapter rows have no original registration timestamp. The first
-- immutable edition timestamp is the earliest provable proxy, not migration time.
UPDATE chapters AS c
   SET registered_at = first_ed.created_at
  FROM (
    SELECT chapter_id, min(created_at) AS created_at
      FROM chapter_versions GROUP BY chapter_id
  ) AS first_ed
 WHERE c.id = first_ed.chapter_id;

CREATE FUNCTION tor_guard_chapter_registration_date() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.registered_at IS DISTINCT FROM OLD.registered_at THEN
        RAISE EXCEPTION 'chapter registration date is immutable'
            USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER chapters_registration_date_immutable
BEFORE UPDATE OF registered_at ON chapters
FOR EACH ROW EXECUTE FUNCTION tor_guard_chapter_registration_date();

ALTER TABLE books
    ADD COLUMN package_only_days SMALLINT
      CHECK (package_only_days BETWEEN 1 AND 365),
    ADD CONSTRAINT books_package_only_days_require_layer
      CHECK (package_only_days IS NULL OR
         (package_only_layer_id IS NOT NULL AND package_only_until IS NULL));

-- Canonical predicate shared by money, offers, and storefront. A NULL
-- historical registration is NOT silently treated as today.
CREATE FUNCTION tor_package_chapter_locked(
    layer_id BIGINT,
    old_book_deadline TIMESTAMPTZ,
    days SMALLINT,
    chapter_registered_at TIMESTAMPTZ
) RETURNS BOOLEAN LANGUAGE SQL STABLE PARALLEL SAFE
AS $$
    SELECT layer_id IS NOT NULL AND CASE
      WHEN days IS NOT NULL THEN
        chapter_registered_at IS NOT NULL AND
        chapter_registered_at + days * INTERVAL '1 day' > now()
      ELSE old_book_deadline IS NULL OR old_book_deadline > now()
    END
$$;

-- Keep H/UI-19 automatic release rules while clearing the new timed mode.
CREATE OR REPLACE FUNCTION tor_reset_unavailable_package_sales() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
    UPDATE books AS b
       SET package_only_layer_id=NULL,
           package_only_until=NULL,
           package_only_days=NULL,
           package_only_revision=b.package_only_revision+1
     WHERE b.package_only_layer_id=NEW.id
       AND (
          NEW.status<>'active'
          OR NEW.current_offer_id IS NULL
          OR NEW.owner_user_id IS DISTINCT FROM OLD.owner_user_id
          OR NOT EXISTS (
              SELECT 1 FROM commerce_layer_version_books vb
              WHERE vb.layer_version_id=NEW.current_version_id
                AND vb.book_id=b.id
          )
       );
    RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION tor_reset_package_sales_after_book_transfer() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.owner_user_id IS DISTINCT FROM OLD.owner_user_id THEN
        NEW.package_only_layer_id := NULL;
        NEW.package_only_until := NULL;
        NEW.package_only_days := NULL;
        NEW.package_only_revision := OLD.package_only_revision + 1;
    END IF;
    RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION tor_reset_package_sales_after_member_change() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
DECLARE
    affected_book BIGINT;
BEGIN
    affected_book := CASE WHEN TG_OP='DELETE' THEN OLD.book_id ELSE NEW.book_id END;
    UPDATE books AS b
       SET package_only_layer_id=NULL,
           package_only_until=NULL,
           package_only_days=NULL,
           package_only_revision=b.package_only_revision+1
     WHERE b.id=affected_book
       AND b.package_only_layer_id IS NOT NULL
       AND (
         NOT EXISTS (
             SELECT 1 FROM book_members m
             WHERE m.book_id=b.id AND m.user_id=b.owner_user_id
               AND m.role='owner' AND m.is_active
         )
         OR EXISTS (
             SELECT 1 FROM book_members m
             WHERE m.book_id=b.id AND m.is_active
               AND (m.user_id<>b.owner_user_id OR m.role<>'owner')
         )
       );
    RETURN NULL;
END;
$$;
