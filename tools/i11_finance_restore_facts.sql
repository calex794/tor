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
-- RECOVERY-10c: a distinct actual D-06 approved refund. The old immutable
-- debit and original earning remain, but the wallet credit, exact reversal,
-- decision and entitlement revocation must all survive the same restore.
DO $refund_restore$
DECLARE
    buyer BIGINT;
    author BIGINT;
    purchase BIGINT;
    chapter BIGINT;
    version BIGINT;
    observed BIGINT;
BEGIN
    SELECT id INTO STRICT buyer FROM users
      WHERE email_normalized='d02_d06_refund_approve_ok@example.test';
    SELECT id INTO STRICT author FROM users
      WHERE email_normalized='d02_owner_d06_refund_approve_ok@example.test';
    SELECT p.id,p.chapter_id,p.version_id INTO STRICT purchase,chapter,version
    FROM chapter_purchase_facts p
    JOIN chapters c ON c.id=p.chapter_id
    JOIN books b ON b.id=c.book_id
    WHERE p.user_id=buyer AND b.source_identity='ci:d02:d06_refund_approve_ok'
      AND b.owner_user_id=author AND c.chapter_no=1
      AND p.amount_minor=300 AND p.currency='RUB';

    IF (SELECT count(*) FROM chapter_purchase_facts WHERE user_id=buyer)<>1
       OR (SELECT count(*) FROM reader_wallet_entries WHERE user_id=buyer)<>2
       OR NOT EXISTS (
           SELECT 1 FROM reader_wallet_accounts
           WHERE user_id=buyer AND balance_minor=1000 AND currency='RUB'
       )
       OR (SELECT count(*) FROM chapter_purchase_commands WHERE user_id=buyer)<>1
       OR (SELECT count(*) FROM finance_direct_refund_requests WHERE purchase_id=purchase)<>1
       OR (SELECT count(*) FROM chapter_purchase_revocations WHERE purchase_id=purchase)<>1
       OR (SELECT count(*) FROM finance_direct_refund_creator_reversals WHERE purchase_id=purchase)<>1
       OR (SELECT count(*) FROM finance_creator_earnings WHERE purchase_id=purchase)<>1
    THEN
        RAISE EXCEPTION 'RECOVERY-10c refund wallet or cardinality witness differs';
    END IF;

    SELECT count(*) INTO observed
    FROM chapter_purchase_facts p
    JOIN finance_chapter_postings fp ON fp.purchase_id=p.id
    JOIN finance_platform_credits pc ON pc.purchase_id=p.id
    JOIN finance_creator_earnings e ON e.purchase_id=p.id AND e.user_id=author
    JOIN finance_direct_refund_postings r ON r.purchase_id=p.id
    JOIN finance_direct_refund_requests q ON q.id=r.request_id
    JOIN finance_direct_refund_creator_reversals cr
      ON cr.purchase_id=p.id AND cr.user_id=author
    JOIN chapter_purchase_revocations v ON v.purchase_id=p.id
    JOIN reader_wallet_entries debit ON debit.purchase_id=p.id
      AND debit.user_id=buyer AND debit.kind='chapter_purchase'
    JOIN reader_wallet_entries credit ON credit.purchase_id=p.id
      AND credit.user_id=buyer AND credit.kind='chapter_refund'
    WHERE p.id=purchase AND fp.gross_minor=300
      AND fp.platform_fee_minor=60 AND fp.creator_pool_minor=240
      AND pc.amount_minor=60 AND e.amount_minor=240
      AND r.buyer_user_id=buyer AND r.currency='RUB' AND r.amount_minor=300
      AND r.platform_reversal_minor=60 AND r.creator_reversal_minor=240
      AND cr.amount_minor=e.amount_minor
      AND q.purchase_id=p.id AND q.user_id=buyer
      AND q.state='approved' AND q.revision=3 AND q.decision_actor_id=author
      AND q.decision_reference=r.reversal_reference
      AND v.reason='refund' AND v.reversal_reference=r.reversal_reference
      AND debit.delta_minor=-300 AND debit.balance_after_minor=700
      AND debit.reference=p.settlement_reference
      AND credit.delta_minor=300 AND credit.balance_after_minor=1000
      AND credit.reference=r.reversal_reference;
    IF observed<>1 OR
       tor_purchase_version_readable(buyer,purchase,chapter,version) IS DISTINCT FROM FALSE
    THEN
        RAISE EXCEPTION 'RECOVERY-10c exact reversal or revoked entitlement differs';
    END IF;
END
$refund_restore$;

-- Append a second deterministic snapshot to the existing one. Both are
-- bound by the same independently retained pre-backup SHA-256.
WITH buyer AS (
    SELECT id FROM users WHERE email_normalized='d02_d06_refund_approve_ok@example.test'
), purchase AS (
    SELECT p.id FROM chapter_purchase_facts p JOIN buyer b ON b.id=p.user_id
)
SELECT jsonb_build_object(
    'refund_wallet', (SELECT to_jsonb(a) FROM reader_wallet_accounts a JOIN buyer b ON b.id=a.user_id),
    'refund_journal', (SELECT jsonb_agg(to_jsonb(e) ORDER BY e.id) FROM reader_wallet_entries e JOIN buyer b ON b.id=e.user_id),
    'refund_purchase', (SELECT jsonb_agg(to_jsonb(p) ORDER BY p.id) FROM chapter_purchase_facts p JOIN buyer b ON b.id=p.user_id),
    'refund_commands', (SELECT jsonb_agg(to_jsonb(q) ORDER BY q.command_key) FROM chapter_purchase_commands q JOIN buyer b ON b.id=q.user_id),
    'refund_original_postings', (SELECT jsonb_agg(to_jsonb(f) ORDER BY f.purchase_id) FROM finance_chapter_postings f JOIN purchase p ON p.id=f.purchase_id),
    'refund_original_platform', (SELECT jsonb_agg(to_jsonb(c) ORDER BY c.purchase_id) FROM finance_platform_credits c JOIN purchase p ON p.id=c.purchase_id),
    'refund_original_creator', (SELECT jsonb_agg(to_jsonb(e) ORDER BY e.purchase_id,e.user_id) FROM finance_creator_earnings e JOIN purchase p ON p.id=e.purchase_id),
    'refund_requests', (SELECT jsonb_agg(to_jsonb(q) ORDER BY q.id) FROM finance_direct_refund_requests q JOIN purchase p ON p.id=q.purchase_id),
    'refund_postings', (SELECT jsonb_agg(to_jsonb(r) ORDER BY r.purchase_id) FROM finance_direct_refund_postings r JOIN purchase p ON p.id=r.purchase_id),
    'refund_creator_reversals', (SELECT jsonb_agg(to_jsonb(r) ORDER BY r.purchase_id,r.user_id) FROM finance_direct_refund_creator_reversals r JOIN purchase p ON p.id=r.purchase_id),
    'refund_revocations', (SELECT jsonb_agg(to_jsonb(v) ORDER BY v.purchase_id) FROM chapter_purchase_revocations v JOIN purchase p ON p.id=v.purchase_id)
)::text;
COMMIT;
