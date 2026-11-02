-- B: count active reader bookmarks for the owner's book statistics.
-- The existing unique(user_id,book_id) index cannot efficiently serve book_id-only counts.
-- A bookmark is not a reading entitlement, purchase, or progress record.
CREATE INDEX reader_library_items_book_stats_idx
    ON reader_library_items (book_id);
