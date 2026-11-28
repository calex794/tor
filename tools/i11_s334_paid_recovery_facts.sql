-- DEPLOY-07c: exact stable witnesses of the two synthetic S334 purchases.
-- This is a CI-only fixture query, not production SQL or a new ACL policy.
-- No session rows, passwords, DEKs, KEKs, wrapped keys or raw object bytes.
BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;
SET LOCAL TIME ZONE 'UTC';

DO $s334$
DECLARE
    target_chapter BIGINT;
    first_version BIGINT;
    second_version BIGINT;
    first_buyer BIGINT;
    second_buyer BIGINT;
    outsider BIGINT;
    first_purchase BIGINT;
    second_purchase BIGINT;
    author_id BIGINT;
    purchase_ref BIGINT;
    buyer_ref BIGINT;
    version_ref BIGINT;
BEGIN
    IF current_setting('transaction_read_only') <> 'on' THEN
        RAISE EXCEPTION 'S334 witness requires read-only transaction';
    END IF;
    SELECT c.id,b.owner_user_id INTO STRICT target_chapter,author_id
      FROM chapters c JOIN books b ON b.id=c.book_id
      WHERE b.source_identity='ci:s334-paid-restore'
        AND b.visibility='draft' AND c.visibility='draft'
        AND c.chapter_no=1 AND c.access_level='paid' AND c.price_amount=175;
    SELECT v.id INTO STRICT first_version
      FROM chapter_versions v JOIN storage_objects s ON s.id=v.storage_object_id
      WHERE v.chapter_id=target_chapter AND v.version_no=1
        AND s.id='f0c0e034-2026-4034-8034-000000000134'::uuid
        AND s.kek_id='content-kek-v1' AND s.state='attached'
        AND s.bucket='tor-private-chapter-ci' AND s.object_kind='chapter';
    SELECT v.id INTO STRICT second_version
      FROM chapter_versions v JOIN storage_objects s ON s.id=v.storage_object_id
      WHERE v.chapter_id=target_chapter AND v.version_no=2
        AND s.id='f0c0e034-2026-4034-8034-000000000234'::uuid
        AND s.kek_id='content-kek-v2' AND s.state='attached'
        AND s.bucket='tor-private-chapter-ci' AND s.object_kind='chapter';
    SELECT id INTO STRICT first_buyer FROM users
      WHERE email_normalized='s334-buyer@tor-ci.example';
    SELECT id INTO STRICT second_buyer FROM users
      WHERE email_normalized='s334-buyer-v2@tor-ci.example';
    SELECT id INTO STRICT outsider FROM users
      WHERE email_normalized='s334-stranger@tor-ci.example';
    SELECT id INTO STRICT first_purchase FROM chapter_purchase_facts
      WHERE user_id=first_buyer AND chapter_id=target_chapter
        AND version_id=first_version AND amount_minor=175 AND currency='RUB';
    SELECT id INTO STRICT second_purchase FROM chapter_purchase_facts
      WHERE user_id=second_buyer AND chapter_id=target_chapter
        AND version_id=second_version AND amount_minor=175 AND currency='RUB';

    IF EXISTS (
        SELECT 1 FROM chapter_purchase_revocations
        WHERE purchase_id IN (first_purchase,second_purchase)
    ) THEN
        RAISE EXCEPTION 'S334 unexpected pre-refund revocation';
    END IF;

    IF first_version=second_version OR first_purchase=second_purchase
       OR (SELECT count(*) FROM chapter_versions WHERE chapter_id=target_chapter)<>2
       OR (SELECT count(*) FROM chapter_purchase_facts WHERE chapter_id=target_chapter)<>2
       OR (SELECT count(*) FROM library_purchased_chapters WHERE chapter_id=target_chapter)<>2
       OR NOT EXISTS (
           SELECT 1 FROM chapter_publications
           WHERE chapter_id=target_chapter AND version_id=second_version
       )
       OR (SELECT count(*) FROM reader_wallet_accounts
           WHERE user_id IN (first_buyer,second_buyer)
             AND balance_minor=425 AND currency='RUB')<>2
       OR (SELECT count(*) FROM reader_wallet_entries
           WHERE purchase_id IN (first_purchase,second_purchase)
             AND kind='chapter_purchase' AND delta_minor=-175 AND balance_after_minor=425)<>2
       -- Exercise the canonical production entitlement SQL function.
       OR tor_purchase_version_readable(first_buyer,first_purchase,target_chapter,first_version) IS DISTINCT FROM TRUE
       OR tor_purchase_version_readable(second_buyer,second_purchase,target_chapter,second_version) IS DISTINCT FROM TRUE
       OR tor_purchase_version_readable(first_buyer,first_purchase,target_chapter,second_version) IS DISTINCT FROM FALSE
       OR tor_purchase_version_readable(first_buyer,second_purchase,target_chapter,second_version) IS DISTINCT FROM FALSE
       OR tor_purchase_version_readable(second_buyer,first_purchase,target_chapter,first_version) IS DISTINCT FROM FALSE
       OR tor_purchase_version_readable(outsider,first_purchase,target_chapter,first_version) IS DISTINCT FROM FALSE
       OR tor_purchase_version_readable(outsider,second_purchase,target_chapter,second_version) IS DISTINCT FROM FALSE
    THEN
        RAISE EXCEPTION 'S334 historical paid entitlement or ledger witness differs';
    END IF;
    -- Verify the actual D-08 posting and frozen terms for EACH encrypted sale.
    FOR purchase_ref,buyer_ref,version_ref IN
        SELECT * FROM (VALUES (first_purchase,first_buyer,first_version),
                              (second_purchase,second_buyer,second_version)) AS expected
    LOOP
        IF (SELECT count(*) FROM reader_wallet_entries WHERE user_id=buyer_ref)<>1
           OR (SELECT count(*) FROM chapter_purchase_commands WHERE user_id=buyer_ref)<>1
           OR NOT EXISTS (
               SELECT 1 FROM reader_wallet_entries e
               JOIN chapter_purchase_facts p ON p.id=e.purchase_id
               WHERE e.user_id=buyer_ref AND e.purchase_id=purchase_ref
                 AND e.kind='chapter_purchase' AND e.delta_minor=-175
                 AND e.balance_after_minor=425 AND e.reference=p.settlement_reference)
           OR NOT EXISTS (
               SELECT 1 FROM chapter_purchase_commands q
               WHERE q.user_id=buyer_ref AND q.purchase_id=purchase_ref
                 AND q.chapter_id=target_chapter AND q.expected_version_id=version_ref
                 AND q.expected_price_minor=175)
           OR NOT EXISTS (
               SELECT 1 FROM library_purchased_chapters l
               WHERE l.purchase_id=purchase_ref AND l.user_id=buyer_ref
                 AND l.chapter_id=target_chapter AND l.active_version_id=version_ref)
           OR (SELECT count(*) FROM finance_chapter_postings WHERE purchase_id=purchase_ref)<>1
           OR (SELECT count(*) FROM finance_creator_earnings WHERE purchase_id=purchase_ref)<>1
           OR (SELECT count(*) FROM finance_platform_credits WHERE purchase_id=purchase_ref)<>1
           OR NOT EXISTS (
               SELECT 1 FROM finance_chapter_postings fp
               JOIN chapter_purchase_facts p ON p.id=fp.purchase_id
               JOIN finance_platform_credits pc ON pc.purchase_id=p.id
               JOIN finance_creator_earnings e ON e.purchase_id=p.id
               WHERE p.id=purchase_ref AND fp.gross_minor=175 AND fp.currency='RUB'
                 AND fp.policy_version='ci-s334-finance-v1'
                 AND fp.platform_fee_minor=35 AND fp.creator_pool_minor=140
                 AND pc.amount_minor=35 AND e.user_id=author_id
                 AND e.share_percent=100 AND e.amount_minor=140
                 AND p.refund_policy_version='ci-s334-refund-7d-v1'
                 AND fp.refund_policy_version=p.refund_policy_version
                 AND fp.refund_deadline_at=p.refund_deadline_at
                 AND fp.refund_deadline_at=p.acquired_at+INTERVAL '604800 seconds'
                 AND e.clear_at=fp.refund_deadline_at)
        THEN
            RAISE EXCEPTION 'S334 encrypted purchase finance witness differs';
        END IF;
    END LOOP;
END
$s334$;

-- Compare actual stable values, not merely count equality. The encrypted
-- object registry is deliberately projected to non-secret receipt fields.
WITH target AS (
    SELECT c.id AS chapter_id,b.id AS book_id
      FROM chapters c JOIN books b ON b.id=c.book_id
      WHERE b.source_identity='ci:s334-paid-restore' AND c.chapter_no=1
), purchases AS (
    SELECT p.* FROM chapter_purchase_facts p JOIN target t ON p.chapter_id=t.chapter_id
)
SELECT jsonb_build_object(
    'target', (SELECT to_jsonb(t) FROM target t),
    'publication', (SELECT to_jsonb(p) FROM chapter_publications p JOIN target t USING(chapter_id)),
    'editions', (SELECT jsonb_agg(jsonb_build_array(v.id,v.version_no,v.storage_object_id,
                    encode(sha256(convert_to(to_jsonb(v)::text,'UTF8')),'hex'))
                                  ORDER BY v.id)
                 FROM chapter_versions v JOIN target t USING(chapter_id)),
    'objects', (SELECT jsonb_agg(jsonb_build_array(
                    s.id,s.object_kind,s.bucket,s.object_key,s.state,s.kek_id,s.size_bytes,
                    encode(s.sha256_ciphertext,'hex'),encode(s.plaintext_sha256,'hex'),
                    s.codec,s.created_at,s.verified_at,s.attached_at,
                    encode(sha256(convert_to(to_jsonb(s)::text,'UTF8')),'hex')) ORDER BY s.id)
                FROM storage_objects s JOIN chapter_versions v ON v.storage_object_id=s.id
                JOIN target t ON t.chapter_id=v.chapter_id),
    'purchases', (SELECT jsonb_agg(to_jsonb(p) ORDER BY p.id) FROM purchases p),
    'library', (SELECT jsonb_agg(to_jsonb(l) ORDER BY l.purchase_id)
                FROM library_purchased_chapters l JOIN target t USING(chapter_id)),
    'wallets', (SELECT jsonb_agg(to_jsonb(w) ORDER BY w.user_id)
                FROM reader_wallet_accounts w JOIN purchases p ON p.user_id=w.user_id),
    'debits', (SELECT jsonb_agg(to_jsonb(e) ORDER BY e.id)
               FROM reader_wallet_entries e JOIN purchases p ON e.purchase_id=p.id),
    'commands', (SELECT jsonb_agg(to_jsonb(q) ORDER BY q.command_key)
                 FROM chapter_purchase_commands q JOIN purchases p ON q.purchase_id=p.id),
    'shares', (SELECT jsonb_agg(to_jsonb(s) ORDER BY s.purchase_id,s.user_id)
               FROM chapter_purchase_share_lines s JOIN purchases p ON s.purchase_id=p.id),
    'postings', (SELECT jsonb_agg(to_jsonb(f) ORDER BY f.purchase_id)
                 FROM finance_chapter_postings f JOIN purchases p ON f.purchase_id=p.id),
    'platform', (SELECT jsonb_agg(to_jsonb(f) ORDER BY f.purchase_id)
                 FROM finance_platform_credits f JOIN purchases p ON f.purchase_id=p.id),
    'creator', (SELECT jsonb_agg(to_jsonb(f) ORDER BY f.purchase_id,f.user_id)
                FROM finance_creator_earnings f JOIN purchases p ON f.purchase_id=p.id)
)::text;
COMMIT;
