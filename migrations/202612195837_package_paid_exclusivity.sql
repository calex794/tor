-- H/UI-19: Author-selected package-only PAID chapter sales.
-- This does not revoke direct purchases, pinned editions, free chapters or
-- historical package terms. Any restriction is checked at money commit.
ALTER TABLE books
    ADD COLUMN package_only_layer_id BIGINT REFERENCES commerce_layers(id) ON DELETE RESTRICT,
    ADD COLUMN package_only_until TIMESTAMPTZ,
    ADD COLUMN package_only_revision BIGINT NOT NULL DEFAULT 0
        CHECK (package_only_revision >= 0),
    ADD CONSTRAINT books_package_only_duration
        CHECK (package_only_layer_id IS NOT NULL OR package_only_until IS NULL);
CREATE INDEX books_package_only_layer_lookup
    ON books(package_only_layer_id) WHERE package_only_layer_id IS NOT NULL;

-- A publisher cannot strand a reader by hiding a required package, removing
-- an exclusive book from the current composition or moving package ownership.
-- This trigger executes under the existing commerce_layers row lock, then
-- updates Book rows; all direct payment paths hold FOR SHARE OF books.
CREATE FUNCTION tor_reset_unavailable_package_sales() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
    UPDATE books AS b
       SET package_only_layer_id=NULL,
           package_only_until=NULL,
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
CREATE TRIGGER trg_tor_reset_unavailable_package_sales
AFTER UPDATE OF status,current_offer_id,current_version_id,owner_user_id
ON commerce_layers
FOR EACH ROW EXECUTE FUNCTION tor_reset_unavailable_package_sales();

-- A book ownership change must not preserve a restriction belonging to the
-- former owner. Historical chapter purchases and package grants are untouched.
CREATE FUNCTION tor_reset_package_sales_after_book_transfer() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.owner_user_id IS DISTINCT FROM OLD.owner_user_id THEN
        NEW.package_only_layer_id := NULL;
        NEW.package_only_until := NULL;
        NEW.package_only_revision := OLD.package_only_revision + 1;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER trg_tor_reset_package_sales_after_book_transfer
BEFORE UPDATE OF owner_user_id ON books
FOR EACH ROW EXECUTE FUNCTION tor_reset_package_sales_after_book_transfer();
