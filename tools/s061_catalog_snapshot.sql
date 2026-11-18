-- The upgrade runs against its own disposable database. Explicit columns
-- capture existing facts without making an additive column itself a diff.
SELECT jsonb_build_object(
  'books', (SELECT jsonb_agg(jsonb_build_array(id, source_identity, title, visibility,
                                              access_level, admin_hold, is_deleted)
                           ORDER BY id) FROM books),
  'chapters', (SELECT jsonb_agg(jsonb_build_array(id, book_id, chapter_no, title, access_level,
                                                price_amount, visibility, admin_hold, is_deleted,
                                                reader_released_at) ORDER BY id) FROM chapters),
  'editions', (SELECT jsonb_agg(jsonb_build_array(chapter_id, id, version_no, body_markdown,
                                                encode(content_sha256,'hex')) ORDER BY id)
               FROM chapter_versions),
  'publications', (SELECT jsonb_agg(jsonb_build_array(chapter_id, version_id) ORDER BY chapter_id)
                   FROM chapter_publications),
  's3_refs', (SELECT jsonb_agg(jsonb_build_array(chapter_id, version_id, bucket, object_key,
                                                encode(sha256,'hex'), size_bytes) ORDER BY version_id)
              FROM free_chapter_storage_objects),
  'saved_free', (SELECT jsonb_agg(jsonb_build_array(user_id, chapter_id, version_id) ORDER BY user_id,chapter_id)
                 FROM reader_saved_free_chapters),
  'layer_archive', (SELECT jsonb_agg(jsonb_build_array(book_id, request_revision, chapter_id, version_id)
                                     ORDER BY book_id,request_revision,chapter_id)
                    FROM book_layer_free_archive_versions),
  'free_position', (SELECT jsonb_agg(jsonb_build_array(user_id, book_id, chapter_id, version_id,
                                                      chapter_no, chapter_revision, block_index,
                                                      char_offset, revision)
                                      ORDER BY user_id,book_id) FROM reader_book_positions),
  'settlement', (SELECT jsonb_agg(jsonb_build_array(id, user_id, chapter_id, version_id, amount_minor,
                                                   currency, settlement_reference) ORDER BY id)
                 FROM chapter_purchase_facts),
  'paid_library', (SELECT jsonb_agg(jsonb_build_array(user_id, chapter_id, active_version_id, purchase_id)
                                     ORDER BY purchase_id) FROM library_purchased_chapters),
  'paid_position', (SELECT jsonb_agg(jsonb_build_array(user_id, purchase_id, chapter_id, version_id,
                                                      block_index, char_offset, revision)
                                      ORDER BY purchase_id) FROM reader_purchased_positions)
);
