-- Bounded keyset first: never join fifty thousand source paths to the public
-- book/chapter projections before applying a page-sized scan window.
-- The caller advances past ineligible paths and continues until 51 visible
-- results (50 + lookahead) or the source is exhausted.
WITH candidate_paths AS MATERIALIZED (
  SELECT path
  FROM flow_seo_pages
  WHERE source_id=$1 AND day >= $2::text::date AND day < $3::text::date AND path>$4
  GROUP BY path
  ORDER BY path
  LIMIT 256
), grouped AS MATERIALIZED (
  SELECT p.path,sum(p.clicks)::bigint AS clicks,sum(p.impressions)::bigint AS impressions,
    CASE WHEN count(*) FILTER(WHERE p.impressions>0 AND p.position IS NULL)=0
      THEN sum(p.position*p.impressions)/nullif(sum(p.impressions),0) END AS position,
    count(DISTINCT p.day) AS measured_days
  FROM candidate_paths c
  JOIN flow_seo_pages p ON p.source_id=$1 AND p.path=c.path
    AND p.day >= $2::text::date AND p.day < $3::text::date
  GROUP BY p.path
), parsed AS (
  SELECT g.*,substring(path from '^/(?:books|reader)/([1-9][0-9]{0,18})$')::numeric AS raw_id
  FROM grouped g
), ids AS (
  SELECT p.*,CASE WHEN raw_id<=9223372036854775807 THEN raw_id::bigint END AS object_id
  FROM parsed p
)
SELECT p.path,CASE WHEN p.path='/' THEN 'Main page' WHEN p.path='/catalog' THEN 'Catalogue'
  ELSE coalesce(b.title,cb.title) END AS title,p.clicks,p.impressions,p.position,p.measured_days,
  (p.path IN ('/','/catalog') OR b.id IS NOT NULL OR cb.id IS NOT NULL) AS eligible
FROM ids p
LEFT JOIN LATERAL (
  SELECT id,title FROM rec_public_books WHERE id=p.object_id AND p.path LIKE '/books/%' OFFSET 0
) b ON true
LEFT JOIN LATERAL (
  SELECT chapter_id,book_id FROM rec_public_chapter_links WHERE chapter_id=p.object_id AND p.path LIKE '/reader/%' OFFSET 0
) c ON true
LEFT JOIN LATERAL (
  SELECT id,title FROM rec_public_books WHERE id=c.book_id OFFSET 0
) cb ON true
ORDER BY p.path
