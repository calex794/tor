-- S020/#486: Shadow and REC are anonymous projections, never a path around
-- the reader and public catalog age and lifecycle gates. Legacy NULL remains
-- non-indexable until its tier is explicitly attested.
-- Additive SQLx migration only; no production execution in this workflow.
CREATE OR REPLACE VIEW shadow_public_books WITH (security_barrier=true) AS
SELECT id,source_identity,target_language,title,visibility,
       author_name,genres,seo_index_owner
FROM books
WHERE shadow_enabled
  AND visibility='public' AND visibility_status='active'
  AND access_level='basic' AND lifecycle_status<>'pending_deletion'
  AND NOT admin_hold AND NOT is_deleted;

-- Paid chapter METADATA may still be public when its Book is basic and opted in.
-- This view must never include a chapter body or a commercial entitlement.
CREATE OR REPLACE VIEW shadow_public_chapters WITH (security_barrier=true) AS
SELECT c.id,c.book_id,c.chapter_no,c.title
FROM chapters AS c
JOIN books AS b ON b.id=c.book_id
JOIN chapter_publications AS p ON p.chapter_id=c.id
WHERE b.shadow_enabled
  AND b.visibility='public' AND b.visibility_status='active'
  AND b.access_level='basic' AND b.lifecycle_status<>'pending_deletion'
  AND NOT b.admin_hold AND NOT b.is_deleted
  AND c.visibility='public' AND NOT c.admin_hold AND NOT c.is_deleted
  AND (c.reader_released_at IS NULL OR c.reader_released_at<=now());

-- REC already required basic. Lifecycle must also block pending deletion.
CREATE OR REPLACE VIEW rec_public_books WITH (security_barrier=true) AS
SELECT b.id,b.title,b.target_language,b.author_name,b.genres,b.tags,b.created_at
FROM books AS b
LEFT JOIN book_recommendation_audiences AS a ON a.book_id=b.id
WHERE b.visibility='public' AND b.visibility_status='active'
  AND b.access_level='basic' AND b.lifecycle_status<>'pending_deletion'
  AND NOT b.admin_hold AND NOT b.is_deleted
  AND COALESCE(a.audience,'general')='general';
