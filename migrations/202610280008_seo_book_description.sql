-- SEO-02: B-owned author-entered book descriptions.
-- Version 202610280007 reserved by Shadow PR #170; merge/order is an I dependency.
ALTER TABLE books
    ADD COLUMN description TEXT NOT NULL DEFAULT ''
        CHECK (char_length(description) <= 4000);
