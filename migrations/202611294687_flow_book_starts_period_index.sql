-- no-transaction
-- I/C FLOW-05: selected book IDs followed by their bounded evidence period.
-- One statement only: SQLx sends this script outside a transaction.
-- No IF NOT EXISTS: an interrupted build must fail until explicitly repaired.
CREATE INDEX CONCURRENTLY reader_chapter_starts_book_period
    ON public.reader_chapter_starts (book_id, started_at);
