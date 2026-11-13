-- Bounded index-ordered keyset, followed by set-based public visibility.
-- Never re-evaluate the security-barrier public projections once per path.
-- The caller advances over hidden paths until 51 visible rows or exhaustion.
WITH candidate_paths AS MATERIALIZED (
  SELECT DISTINCT ON (path) path
  FROM flow_seo_pages
  WHERE source_id=$1 AND day >= $2::text::date AND day < $3::text::date AND path>$4
  ORDER BY path
  LIMIT 64
), grouped AS MATERIALIZED (
  SELECT p.path,sum(p.clicks)::bigint AS clicks,sum(p.impressions)::bigint AS impressions,
    CASE WHEN count(*) FILTER(WHERE p.impressions>0 AND p.position IS NULL)=0
      THEN sum(p.position*p.impressions)/nullif(sum(p.impressions),0) END AS position,
    count(DISTINCT p.day) AS measured_days
  FROM candidate_paths c
  JOIN flow_seo_pages p ON p.source_id=$1 AND p.path=c.path
    AND p.day >= $2::text::date AND p.day < $3::text::date
  GROUP BY p.path
), ids AS MATERIALIZED (
  SELECT g.*,
    CASE WHEN substring(g.path from '^/(?:books|reader)/([1-9][0-9]{0,18})$')::numeric
      <=9223372036854775807
      THEN substring(g.path from '^/(?:books|reader)/([1-9][0-9]{0,18})$')::bigint
    END AS object_id
  FROM grouped g
), candidate_public_chapters AS MATERIALIZED (
  SELECT DISTINCT c.chapter_id,c.book_id
  FROM rec_public_chapter_links c
  JOIN ids i ON i.object_id=c.chapter_id AND i.path LIKE '/reader/%'
), candidate_book_ids AS MATERIALIZED (
  SELECT i.object_id AS book_id FROM ids i WHERE i.path LIKE '/books/%'
  UNION
  SELECT c.book_id FROM candidate_public_chapters c
), visible_books AS MATERIALIZED (
  SELECT b.id,b.title
  FROM rec_public_books b
  JOIN candidate_book_ids k ON k.book_id=b.id
)
SELECT p.path,
  CASE WHEN p.path='/' THEN 'Main page' WHEN p.path='/catalog' THEN 'Catalogue'
    ELSE coalesce(b.title,cb.title) END AS title,
  p.clicks,p.impressions,p.position,p.measured_days,
  (p.path IN ('/','/catalog') OR b.id IS NOT NULL OR cb.id IS NOT NULL) AS eligible
FROM ids p
LEFT JOIN visible_books b ON b.id=p.object_id AND p.path LIKE '/books/%'
LEFT JOIN candidate_public_chapters c ON c.chapter_id=p.object_id AND p.path LIKE '/reader/%'
LEFT JOIN visible_books cb ON cb.id=c.book_id
ORDER BY p.path
