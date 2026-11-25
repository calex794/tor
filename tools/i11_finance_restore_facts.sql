-- RECOVERY-10a: read-only witness for the existing real D-08 single checkout.
-- This synthetic fixture starts with 1000 minor RUB units; its initial
-- balance is not evidence of a verified funding journal. No production SQL.
BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;
SET LOCAL TIME ZONE 'UTC';

DO $finance_restore$
DECLARE
    buyer BIGINT;
    author BIGINT;
    book BIGINT;
    purchase BIGINT;
    observed BIGINT;
BEGIN
    SELECT id INTO STRICT buyer FROM users
      WHERE email_normalized='d08_buyer_single@example.test';
    SELECT id INTO STRICT author FROM users
      WHERE email_normalized='d08_owner_single@example.test';
    SELECT id INTO STRICT book FROM books
      WHERE source_identity='ci:d08:single' AND owner_user_id=author;

    SELECT p.id INTO STRICT purchase
    FROM chapter_purchase_facts p
    JOIN chapters c ON c.id=p.chapter_id
    JOIN chapter_versions v ON v.id=p.version_id AND v.chapter_id=c.id
    WHERE p.user_id=buyer AND c.book_id=book AND c.chapter_no=1
      AND v.version_no=1 AND p.amount_minor=250 AND p.currency='RUB';

    IF (SELECT count(*) FROM chapter_purchase_facts WHERE user_id=buyer) <> 1
       OR NOT EXISTS (
           SELECT 1 FROM reader_wallet_accounts
           WHERE user_id=buyer AND balance_minor=750 AND currency='RUB'
       )
       OR (SELECT count(*) FROM reader_wallet_entries WHERE user_id=buyer) <> 1
       OR NOT EXISTS (
           SELECT 1 FROM reader_wallet_entries e
           JOIN chapter_purchase_facts p ON p.id=e.purchase_id
           WHERE e.user_id=buyer AND e.purchase_id=purchase
             AND e.kind='chapter_purchase' AND e.delta_minor=-250
             AND e.balance_after_minor=750 AND e.reference=p.settlement_reference
       )
       OR (SELECT count(*) FROM chapter_purchase_commands WHERE user_id=buyer) <> 1
       OR NOT EXISTS (
           SELECT 1 FROM chapter_purchase_commands q
           JOIN chapter_purchase_facts p ON p.id=q.purchase_id
           WHERE q.user_id=buyer AND q.purchase_id=purchase
             AND q.chapter_id=p.chapter_id AND q.expected_version_id=p.version_id
             AND q.expected_price_minor=250
       )
    THEN
        RAISE EXCEPTION 'RECOVERY-10a wallet, debit or idempotency witness differs';
    END IF;

    SELECT count(*) INTO observed
    FROM finance_chapter_postings fp
    JOIN chapter_purchase_facts p ON p.id=fp.purchase_id
    JOIN finance_platform_credits pc ON pc.purchase_id=p.id
    JOIN finance_creator_earnings e ON e.purchase_id=p.id
    WHERE p.id=purchase AND fp.gross_minor=250
      AND fp.platform_fee_minor=50 AND fp.creator_pool_minor=200
      AND pc.amount_minor=50 AND e.user_id=author AND e.amount_minor=200
      AND fp.currency='RUB'
      AND fp.refund_policy_version=p.refund_policy_version
      AND fp.refund_deadline_at=p.refund_deadline_at
      AND e.clear_at=fp.refund_deadline_at;
    IF observed <> 1
       OR (SELECT count(*) FROM finance_creator_earnings WHERE purchase_id=purchase) <> 1
       OR (SELECT count(*) FROM finance_platform_credits WHERE purchase_id=purchase) <> 1
    THEN
        RAISE EXCEPTION 'RECOVERY-10a exact financial posting witness differs';
    END IF;
END
$finance_restore$;

-- Stable complete row snapshots of the selected synthetic checkout. Compare
-- exact IDs, currency, reference, timestamps and amounts, not just totals.
WITH buyer AS (
    SELECT id FROM users WHERE email_normalized='d08_buyer_single@example.test'
), purchase AS (
    SELECT p.id FROM chapter_purchase_facts p JOIN buyer b ON b.id=p.user_id
)
SELECT jsonb_build_object(
    'wallet', (SELECT to_jsonb(a) FROM reader_wallet_accounts a JOIN buyer b ON b.id=a.user_id),
    'journal', (SELECT jsonb_agg(to_jsonb(e) ORDER BY e.id) FROM reader_wallet_entries e JOIN buyer b ON b.id=e.user_id),
    'purchases', (SELECT jsonb_agg(to_jsonb(p) ORDER BY p.id) FROM chapter_purchase_facts p JOIN buyer b ON b.id=p.user_id),
    'commands', (SELECT jsonb_agg(to_jsonb(q) ORDER BY q.command_key) FROM chapter_purchase_commands q JOIN buyer b ON b.id=q.user_id),
    'postings', (SELECT jsonb_agg(to_jsonb(f) ORDER BY f.purchase_id) FROM finance_chapter_postings f JOIN purchase p ON p.id=f.purchase_id),
    'platform', (SELECT jsonb_agg(to_jsonb(c) ORDER BY c.purchase_id) FROM finance_platform_credits c JOIN purchase p ON p.id=c.purchase_id),
    'creator', (SELECT jsonb_agg(to_jsonb(e) ORDER BY e.purchase_id,e.user_id) FROM finance_creator_earnings e JOIN purchase p ON p.id=e.purchase_id)
)::text;
COMMIT;
