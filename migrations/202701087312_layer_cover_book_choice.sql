-- An author's cosmetic selection of an existing public Book cover.
-- This changes no package version, paid grant, or moderated uploaded image.
ALTER TABLE commerce_layers
  ADD COLUMN cover_book_id BIGINT REFERENCES books(id) ON DELETE SET NULL;
