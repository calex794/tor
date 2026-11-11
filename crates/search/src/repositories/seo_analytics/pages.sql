-- Group only the requested provider/range, then use numeric indexed identities.
-- Never concatenate the catalogue key into an unindexed join expression.
WITH grouped AS (
  SELECT path,sum(clicks)::bigint AS clicks,sum(impressions)::bigint AS impressions,
    CASE WHEN count(*) FILTER(WHERE impressions>0 AND position IS NULL)=0
      THEN sum(position*impressions)/nullif(sum(impressions),0) END AS position,
    count(DISTINCT day) AS measured_days
  FROM flow_seo_pages
  WHERE source_id=$1 AND day >= $2::text::date AND day < $3::text::date AND path>$4
  GROUP BY path
), parsed AS (
  SELECT g.*,substring(path from '^/(?:books|reader)/([1-9][0-9]{0,18})$')::numeric AS raw_id
  FROM grouped g
), ids AS (
  SELECT p.*,CASE WHEN raw_id<=9223372036854775807 THEN raw_id::bigint END AS object_id
  FROM parsed p
)
SELECT p.path,CASE WHEN p.path='/' THEN 'Main page' WHEN p.path='/catalog' THEN 'Catalogue'
  ELSE coalesce(b.title,cb.title) END AS title,p.clicks,p.impressions,p.position,p.measured_days
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
WHERE p.path IN ('/','/catalog') OR b.id IS NOT NULL OR cb.id IS NOT NULL
ORDER BY p.path LIMIT 51
