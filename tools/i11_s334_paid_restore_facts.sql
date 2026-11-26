-- RECOVERY-10e-b2: read-only witness for the existing S334 actual purchases.
-- The caller opens a repeatable-read read-only transaction. No real money,
-- reader sessions, raw key material or plaintext are included in this output.
SET LOCAL TIME ZONE 'UTC';

DO $s334_witness$
DECLARE
    target BIGINT;
    author BIGINT;
    current_v2 BIGINT;
    expected RECORD;
    actual RECORD;
BEGIN
    IF current_setting('transaction_read_only') <> 'on' THEN
        RAISE EXCEPTION 'S334 witness requires read-only transaction';
    END IF;

    SELECT c.id,b.owner_user_id,p.version_id INTO STRICT target,author,current_v2
    FROM books b JOIN chapters c ON c.book_id=b.id
    JOIN chapter_publications p ON p.chapter_id=c.id
    WHERE b.source_identity='ci:s334-paid-restore' AND b.visibility='draft'
      AND c.chapter_no=1 AND c.visibility='draft'
      AND c.access_level='paid' AND c.price_amount=175;

    IF (SELECT count(*) FROM chapter_versions WHERE chapter_id=target)<>2
       OR (SELECT count(*) FROM chapter_purchase_facts WHERE chapter_id=target)<>2
       OR (SELECT count(*) FROM library_purchased_chapters WHERE chapter_id=target)<>2
    THEN
        RAISE EXCEPTION 'S334 encrypted edition or purchase cardinality differs';
    END IF;

    FOR expected IN SELECT * FROM (VALUES
      (1,'s334-buyer@tor-ci.example','content-kek-v1','f0c0e034-2026-4034-8034-000000000134'),
      (2,'s334-buyer-v2@tor-ci.example','content-kek-v2','f0c0e034-2026-4034-8034-000000000234')
    ) AS e(version_no,email,kek,object_id)
    LOOP
        SELECT p.id,p.user_id,p.version_id INTO STRICT actual
        FROM chapter_versions v
        JOIN storage_objects o ON o.id=v.storage_object_id
        JOIN chapter_purchase_facts p ON p.chapter_id=v.chapter_id AND p.version_id=v.id
        JOIN users u ON u.id=p.user_id
        JOIN library_purchased_chapters l ON l.purchase_id=p.id
          AND l.user_id=p.user_id AND l.active_version_id=v.id AND l.chapter_id=v.chapter_id
        WHERE v.chapter_id=target AND v.version_no=expected.version_no
          AND u.email_normalized=expected.email
          AND o.id=expected.object_id::uuid AND o.kek_id=expected.kek
          AND o.state='attached' AND o.object_kind='chapter' AND o.codec='gzip'
          AND o.bucket='tor-private-chapter-ci'
          AND o.object_key='private/chapter-versions/'||expected.object_id||'.enc'
          AND o.size_bytes>=17 AND o.verified_at IS NOT NULL AND o.attached_at IS NOT NULL
          AND p.amount_minor=175 AND p.currency='RUB'
          AND ((expected.version_no=1 AND v.id<>current_v2)
            OR (expected.version_no=2 AND v.id=current_v2));

        IF tor_purchase_version_readable(actual.user_id,actual.id,target,actual.version_id)
              IS DISTINCT FROM TRUE
           OR EXISTS (SELECT 1 FROM chapter_purchase_revocations WHERE purchase_id=actual.id)
        THEN
            RAISE EXCEPTION 'S334 encrypted purchase entitlement witness differs';
        END IF;

        IF (SELECT count(*) FROM reader_wallet_entries WHERE user_id=actual.user_id)<>1
           OR (SELECT count(*) FROM chapter_purchase_commands WHERE user_id=actual.user_id)<>1
           OR NOT EXISTS (SELECT 1 FROM reader_wallet_accounts w
               WHERE w.user_id=actual.user_id AND w.balance_minor=425 AND w.currency='RUB')
           OR NOT EXISTS (
               SELECT 1 FROM reader_wallet_entries e JOIN chapter_purchase_facts p ON p.id=e.purchase_id
               WHERE e.user_id=actual.user_id AND e.purchase_id=actual.id
                 AND e.kind='chapter_purchase' AND e.delta_minor=-175
                 AND e.balance_after_minor=425 AND e.reference=p.settlement_reference)
           OR NOT EXISTS (
               SELECT 1 FROM chapter_purchase_commands q
               WHERE q.user_id=actual.user_id AND q.purchase_id=actual.id AND q.chapter_id=target
                 AND q.expected_version_id=actual.version_id AND q.expected_price_minor=175)
           OR (SELECT count(*) FROM finance_creator_earnings WHERE purchase_id=actual.id)<>1
           OR (SELECT count(*) FROM finance_platform_credits WHERE purchase_id=actual.id)<>1
           OR NOT EXISTS (
               SELECT 1 FROM finance_chapter_postings fp
               JOIN chapter_purchase_facts p ON p.id=fp.purchase_id
               JOIN finance_platform_credits pc ON pc.purchase_id=p.id
               JOIN finance_creator_earnings e ON e.purchase_id=p.id
               WHERE p.id=actual.id AND fp.gross_minor=175 AND fp.currency='RUB'
                 AND fp.platform_fee_minor=35 AND fp.creator_pool_minor=140
                 AND pc.amount_minor=35 AND e.user_id=author AND e.amount_minor=140
                 AND fp.refund_policy_version=p.refund_policy_version
                 AND fp.refund_deadline_at=p.refund_deadline_at AND e.clear_at=fp.refund_deadline_at)
        THEN
            RAISE EXCEPTION 'S334 encrypted purchase finance witness differs';
        END IF;
    END LOOP;
END
$s334_witness$;

WITH target AS (
    SELECT c.id FROM books b JOIN chapters c ON c.book_id=b.id
    WHERE b.source_identity='ci:s334-paid-restore' AND c.chapter_no=1
), purchases AS (
    SELECT p.* FROM chapter_purchase_facts p JOIN target t ON p.chapter_id=t.id
), editions AS (
    SELECT v.* FROM chapter_versions v JOIN target t ON v.chapter_id=t.id
)
SELECT jsonb_build_object(
    'publication', (SELECT to_jsonb(p) FROM chapter_publications p JOIN target t ON p.chapter_id=t.id),
    'editions', (SELECT jsonb_agg(jsonb_build_array(v.id,encode(sha256(convert_to(to_jsonb(v)::text,'UTF8')),'hex')) ORDER BY v.id) FROM editions v),
    'registry', (SELECT jsonb_agg(jsonb_build_array(o.id,encode(sha256(convert_to(to_jsonb(o)::text,'UTF8')),'hex')) ORDER BY o.id) FROM storage_objects o JOIN editions v ON v.storage_object_id=o.id),
    'purchases', (SELECT jsonb_agg(to_jsonb(p) ORDER BY p.id) FROM purchases p),
    'library', (SELECT jsonb_agg(to_jsonb(l) ORDER BY l.purchase_id) FROM library_purchased_chapters l JOIN purchases p ON p.id=l.purchase_id),
    'wallets', (SELECT jsonb_agg(to_jsonb(w) ORDER BY w.user_id) FROM reader_wallet_accounts w WHERE w.user_id IN (SELECT user_id FROM purchases)),
    'journal', (SELECT jsonb_agg(to_jsonb(e) ORDER BY e.id) FROM reader_wallet_entries e JOIN purchases p ON p.id=e.purchase_id),
    'commands', (SELECT jsonb_agg(to_jsonb(q) ORDER BY q.command_key) FROM chapter_purchase_commands q JOIN purchases p ON p.id=q.purchase_id),
    'postings', (SELECT jsonb_agg(to_jsonb(f) ORDER BY f.purchase_id) FROM finance_chapter_postings f JOIN purchases p ON p.id=f.purchase_id),
    'platform', (SELECT jsonb_agg(to_jsonb(f) ORDER BY f.purchase_id) FROM finance_platform_credits f JOIN purchases p ON p.id=f.purchase_id),
    'creator', (SELECT jsonb_agg(to_jsonb(f) ORDER BY f.purchase_id,f.user_id) FROM finance_creator_earnings f JOIN purchases p ON p.id=f.purchase_id)
)::text;
