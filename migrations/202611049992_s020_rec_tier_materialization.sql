-- S020 / REC: materialize recommendation candidates for all explicitly classified
-- visible Book tiers. This is a server-side candidate pool, NOT an anonymous
-- catalog or a grant of reader/chapter/paid access.
-- The final session-derived rec_actor_books gate still decides what to return.
-- Keep rec_public_books unchanged: public SEO / anonymous metadata stay basic-only.
CREATE VIEW rec_materializable_books WITH (security_barrier=true) AS
SELECT b.id,b.title,b.target_language,b.author_name,b.genres,b.tags,
       b.created_at,b.access_level
FROM books AS b
LEFT JOIN book_recommendation_audiences AS a ON a.book_id=b.id
WHERE b.visibility='public' AND b.visibility_status='active'
  AND b.lifecycle_status<>'pending_deletion'
  AND NOT b.admin_hold AND NOT b.is_deleted
  AND (
      (b.access_level='basic' AND COALESCE(a.audience,'general')='general')
      OR (b.access_level IN ('adult','pro_adult')
          AND COALESCE(a.audience,'general') IN ('general','adult'))
  );
