-- Advance through indexed paths before aggregating metrics. The report needs only
-- the first 51 *visible* paths; limiting raw candidates would hide later pages.
-- Keep numeric identities and public-only lookups (never concatenate index keys).
WITH visible_keys AS MATERIALIZED (
  SELECT v.path,
    CASE WHEN v.path='/' THEN 'Main page'
         WHEN v.path='/catalog' THEN 'Catalogue'
         ELSE coalesce(b.title,cb.title) END AS title
  FROM (
    SELECT DISTINCT ON (path) path
    FROM flow_seo_pages
    WHERE source_id=$1 AND day >= $2::text::date AND day < $3::text::date AND path>$4
    ORDER BY path
  ) v
  LEFT JOIN LATERAL (
    SELECT substring(v.path from '^/(?:books|reader)/([1-9][0-9]{0,18})$')::numeric AS raw_id
  ) parsed ON true
  LEFT JOIN LATERAL (
    SELECT CASE WHEN parsed.raw_id<=9223372036854775807 THEN parsed.raw_id::bigint END AS object_id
  ) ids ON true
  LEFT JOIN LATERAL (
    SELECT id,title FROM rec_public_books
    WHERE id=ids.object_id AND v.path LIKE '/books/%' OFFSET 0
  ) b ON true
  LEFT JOIN LATERAL (
    SELECT chapter_id,book_id FROM rec_public_chapter_links
    WHERE chapter_id=ids.object_id AND v.path LIKE '/reader/%' OFFSET 0
  ) c ON true
  LEFT JOIN LATERAL (
    SELECT id,title FROM rec_public_books WHERE id=c.book_id OFFSET 0
  ) cb ON true
  WHERE v.path IN ('/','/catalog') OR b.id IS NOT NULL OR cb.id IS NOT NULL
  ORDER BY v.path LIMIT 51
)
SELECT v.path,v.title,
  sum(p.clicks)::bigint AS clicks,
  sum(p.impressions)::bigint AS impressions,
  CASE WHEN count(*) FILTER(WHERE p.impressions>0 AND p.position IS NULL)=0
    THEN sum(p.position*p.impressions)/nullif(sum(p.impressions),0) END AS position,
  count(DISTINCT p.day) AS measured_days
FROM visible_keys v
JOIN flow_seo_pages p
  ON p.source_id=$1 AND p.path=v.path
  AND p.day >= $2::text::date AND p.day < $3::text::date
GROUP BY v.path,v.title
ORDER BY v.path
