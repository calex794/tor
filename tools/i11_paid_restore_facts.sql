-- FND-05 INTERNAL: read-only snapshot across pg_dump/pg_restore. The
-- invariant check executes both before backup and after restoring the archive.
DO $i11_paid_restore$
DECLARE
    witness_count BIGINT;
BEGIN
    SELECT count(*) INTO witness_count
    FROM books b
    JOIN chapters c ON c.book_id=b.id
    JOIN chapter_versions v1 ON v1.chapter_id=c.id AND v1.version_no=1
    JOIN chapter_versions v2 ON v2.chapter_id=c.id AND v2.version_no=2
    JOIN chapter_publications p ON p.chapter_id=c.id AND p.version_id=v2.id
    JOIN chapter_purchase_facts pf ON pf.chapter_id=c.id AND pf.version_id=v1.id
    JOIN library_purchased_chapters l ON l.purchase_id=pf.id
         AND l.chapter_id=c.id AND l.user_id=pf.user_id AND l.active_version_id=v1.id
    JOIN reader_purchased_positions rp ON rp.purchase_id=pf.id
         AND rp.chapter_id=c.id AND rp.user_id=pf.user_id AND rp.version_id=v1.id
    WHERE b.source_identity='ci:i11:backup-public'
      AND b.visibility='public'
      AND c.chapter_no=1 AND c.visibility='public' AND c.access_level='paid'
      AND c.price_amount=157
      AND pf.settlement_reference='ci:i11:paid-v1-purchase'
      AND pf.amount_minor=157 AND pf.currency='RUB'
      AND rp.block_index=2 AND rp.char_offset=3
      AND (SELECT count(*) FROM chapter_versions WHERE chapter_id=c.id)=2
      AND v1.body_markdown='I11 purchased paid version V1'
      AND v2.body_markdown='I11 current paid version V2'
      AND v1.content_sha256=sha256(convert_to(v1.body_markdown, 'UTF8'))
      AND v2.content_sha256=sha256(convert_to(v2.body_markdown, 'UTF8'));
    IF witness_count <> 1 THEN
        RAISE EXCEPTION
            'FND05 restored paid V1/V2, historical purchase or position differs';
    END IF;
END
$i11_paid_restore$;

SELECT jsonb_build_object(
    'book', jsonb_build_array(b.id,b.source_identity,b.visibility),
    'chapter', jsonb_build_array(c.id,c.chapter_no,c.access_level,
                                 c.price_amount,c.visibility),
    'current_version',p.version_id,
    'editions', (
        SELECT jsonb_agg(
            jsonb_build_array(v.id,v.version_no,v.body_markdown,
                              encode(v.content_sha256,'hex')) ORDER BY v.id
        )
        FROM chapter_versions v WHERE v.chapter_id=c.id
    ),
    'purchases',(
        SELECT jsonb_agg(jsonb_build_array(pf.id,pf.user_id,pf.chapter_id,
                     pf.version_id,pf.settlement_reference,pf.amount_minor,pf.currency)
                     ORDER BY pf.id)
        FROM chapter_purchase_facts pf WHERE pf.chapter_id=c.id
    ),
    'pinned_library',(
        SELECT jsonb_agg(jsonb_build_array(l.user_id,l.chapter_id,
                      l.active_version_id,l.purchase_id) ORDER BY l.purchase_id)
        FROM library_purchased_chapters l WHERE l.chapter_id=c.id
    ),
    'reader_positions',(
        SELECT jsonb_agg(jsonb_build_array(r.user_id,r.purchase_id,r.chapter_id,
                      r.version_id,r.block_index,r.char_offset,r.revision)
                      ORDER BY r.purchase_id)
        FROM reader_purchased_positions r WHERE r.chapter_id=c.id
    )
)::text
FROM books b
JOIN chapters c ON c.book_id=b.id
JOIN chapter_publications p ON p.chapter_id=c.id
WHERE b.source_identity='ci:i11:backup-public' AND c.chapter_no=1;
