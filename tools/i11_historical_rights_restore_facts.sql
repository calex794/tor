-- RECOVERY-10d: read-only database witness for the existing DROP-07
-- actual saved FREE and Layer checkout/deletion fixture. Run inside the
-- caller's repeatable-read read-only snapshot. No session hashes or secrets.
DO $historical_rights$
DECLARE
    book BIGINT;
    chapter BIGINT;
    version BIGINT;
    owner_id BIGINT;
    saved_id BIGINT;
    buyer_id BIGINT;
    revoked_id BIGINT;
    unrelated_id BIGINT;
    buyer_term BIGINT;
    revoked_term BIGINT;
    paid RECORD;
BEGIN
    SELECT b.id,c.id,v.id,b.owner_user_id INTO STRICT book,chapter,version,owner_id
    FROM books b JOIN chapters c ON c.book_id=b.id
    JOIN chapter_versions v ON v.chapter_id=c.id
    WHERE b.source_identity='test:drop07:backup' AND c.chapter_no=1 AND v.version_no=1
      AND b.lifecycle_status='ordinary_deleted' AND b.ordinary_deleted_at IS NOT NULL
      AND NOT b.admin_hold AND NOT b.is_deleted
      AND c.access_level='free' AND c.price_amount=0
      AND c.visibility='public' AND NOT c.admin_hold AND NOT c.is_deleted
      AND v.body_markdown IS NULL
      AND v.content_sha256=sha256(convert_to('DROP-07 recovered historical FREE V1','UTF8'));
    SELECT id INTO STRICT saved_id FROM users WHERE email_normalized='drop07-saved@example.invalid' AND account_state='active';
    SELECT id INTO STRICT buyer_id FROM users WHERE email_normalized='drop07-buyer@example.invalid' AND account_state='active';
    SELECT id INTO STRICT revoked_id FROM users WHERE email_normalized='drop07-revoked@example.invalid' AND account_state='active';
    SELECT id INTO STRICT unrelated_id FROM users WHERE email_normalized='drop07-unrelated@example.invalid' AND account_state='active';

    IF (SELECT count(*) FROM reader_saved_free_chapters WHERE chapter_id=chapter)<>1
       OR NOT EXISTS (
          SELECT 1 FROM reader_saved_free_chapters
          WHERE user_id=saved_id AND chapter_id=chapter AND version_id=version
       )
       OR EXISTS (
          SELECT 1 FROM free_chapter_storage_objects WHERE chapter_id=chapter AND version_id=version
       )
       OR (SELECT count(*) FROM book_layer_free_archive_versions WHERE chapter_id=chapter)<>1
       OR NOT EXISTS (
          SELECT 1 FROM book_layer_free_archive_versions a
          JOIN book_ordinary_deletion_requests q
            ON q.book_id=a.book_id AND q.request_revision=a.request_revision
          JOIN book_ordinary_deletion_decisions decision
            ON decision.book_id=q.book_id AND decision.request_revision=q.request_revision
          JOIN books b ON b.id=a.book_id
          WHERE a.book_id=book AND a.chapter_id=chapter AND a.version_id=version
            AND q.owner_user_id=owner_id AND decision.decision='approve'
            AND b.lifecycle_revision=q.request_revision+1
       )
       OR (SELECT count(*)
           FROM free_edition_active_storage a
           JOIN free_edition_storage_generations g USING (chapter_id,version_id,generation)
           JOIN free_storage_packs p ON p.id=g.pack_id
           JOIN chapter_versions v ON v.id=g.version_id AND v.chapter_id=g.chapter_id
           WHERE g.chapter_id=chapter AND g.version_id=version AND g.placement_kind='pack'
             AND g.content_sha256=v.content_sha256 AND g.frame_offset=0
             AND p.bucket='tor-dr-ci-book-pack'
             AND p.object_key='private/free-packs/import/ci/drop07-restore-v1.zst'
             AND p.manifest_key='private/free-packs/import/ci/drop07-restore-v1.manifest.json')<>1
    THEN
        RAISE EXCEPTION 'RECOVERY-10d historical FREE pin, deletion or PACK registry differs';
    END IF;

    SELECT t.id INTO STRICT buyer_term FROM commerce_layer_terms t
    JOIN commerce_layers l ON l.id=t.layer_id
    JOIN commerce_layer_versions v ON v.id=t.access_origin_version_id AND v.layer_id=t.layer_id
    JOIN commerce_layer_version_books vb ON vb.layer_version_id=v.id AND vb.book_id=book
    JOIN book_ordinary_deletion_requests q ON q.book_id=book
    WHERE t.user_id=buyer_id AND l.owner_user_id=owner_id AND l.title='Recovery Layer'
      AND t.amount_minor=90 AND t.purchased_at<q.requested_at AND v.created_at<q.requested_at
      AND t.started_at<=now() AND (t.expires_at IS NULL OR t.expires_at>now());
    SELECT t.id INTO STRICT revoked_term FROM commerce_layer_terms t
    JOIN commerce_layers l ON l.id=t.layer_id
    JOIN commerce_layer_versions v ON v.id=t.access_origin_version_id AND v.layer_id=t.layer_id
    JOIN commerce_layer_version_books vb ON vb.layer_version_id=v.id AND vb.book_id=book
    WHERE t.user_id=revoked_id AND l.owner_user_id=owner_id AND l.title='Recovery Layer'
      AND t.amount_minor=90;
    IF buyer_term=revoked_term
       OR EXISTS (SELECT 1 FROM commerce_layer_term_revocations WHERE term_id=buyer_term)
       OR NOT EXISTS (
           SELECT 1 FROM commerce_layer_term_revocations
           WHERE term_id=revoked_term AND reason='legal_block'
             AND reversal_reference='ci:drop07:layer-revoked'
       )
       OR EXISTS (
           SELECT 1 FROM commerce_layer_terms t JOIN commerce_layer_versions v ON v.layer_id=t.layer_id
           JOIN commerce_layer_version_books vb ON vb.layer_version_id=v.id AND vb.book_id=book
           WHERE t.user_id IN (saved_id,unrelated_id)
       )
       OR (SELECT count(*) FROM reader_wallet_accounts
           WHERE user_id IN (buyer_id,revoked_id) AND balance_minor=410 AND currency='RUB')<>2
       OR (SELECT count(*) FROM reader_wallet_entries
           WHERE layer_term_id IN (buyer_term,revoked_term) AND kind='layer_purchase'
             AND delta_minor=-90 AND balance_after_minor=410)<>2
    THEN
        RAISE EXCEPTION 'RECOVERY-10d Layer term, debit or revocation differs';
    END IF;

    -- Use the actual paid entitlement SQL function, not a second policy.
    SELECT p.id,p.user_id,p.chapter_id,p.version_id,current.version_id AS current_version
      INTO STRICT paid
    FROM chapter_purchase_facts p
    JOIN chapter_publications current ON current.chapter_id=p.chapter_id
    WHERE p.settlement_reference='ci:i11:paid-v1-purchase';
    IF paid.version_id=paid.current_version
       OR tor_purchase_version_readable(paid.user_id,paid.id,paid.chapter_id,paid.version_id) IS DISTINCT FROM TRUE
       OR tor_purchase_version_readable(paid.user_id,paid.id,paid.chapter_id,paid.current_version) IS DISTINCT FROM FALSE
       OR tor_purchase_version_readable(unrelated_id,paid.id,paid.chapter_id,paid.version_id) IS DISTINCT FROM FALSE
    THEN
        RAISE EXCEPTION 'RECOVERY-10d paid pinned edition or unauthorized denial differs';
    END IF;
END
$historical_rights$;

-- Exact stable database records of the existing fixture; no application
-- session rows, password hashes or actual object bytes.
WITH target AS (
    SELECT b.id AS book_id,c.id AS chapter_id,b.owner_user_id
    FROM books b JOIN chapters c ON c.book_id=b.id
    WHERE b.source_identity='test:drop07:backup' AND c.chapter_no=1
), selected_layers AS (
    SELECT l.id FROM commerce_layers l JOIN target a ON a.owner_user_id=l.owner_user_id
    WHERE l.title='Recovery Layer'
), terms AS (
    SELECT t.* FROM commerce_layer_terms t JOIN selected_layers l ON l.id=t.layer_id
), generations AS (
    SELECT g.* FROM free_edition_storage_generations g JOIN target a ON a.chapter_id=g.chapter_id
)
SELECT jsonb_build_object(
    'book_lifecycle', (SELECT jsonb_build_array(b.id,b.lifecycle_status,b.lifecycle_revision,b.ordinary_deleted_at,b.visibility_status,b.admin_hold,b.is_deleted) FROM books b JOIN target a ON a.book_id=b.id),
    'free_versions', (SELECT jsonb_agg(jsonb_build_array(v.id,v.version_no,encode(v.content_sha256,'hex')) ORDER BY v.id) FROM chapter_versions v JOIN target a ON a.chapter_id=v.chapter_id),
    'saved_pins', (SELECT jsonb_agg(to_jsonb(s) ORDER BY s.user_id) FROM reader_saved_free_chapters s JOIN target a ON a.chapter_id=s.chapter_id),
    'archive_pins', (SELECT jsonb_agg(to_jsonb(s) ORDER BY s.request_revision,s.version_id) FROM book_layer_free_archive_versions s JOIN target a ON a.chapter_id=s.chapter_id),
    'delete_requests', (SELECT jsonb_agg(to_jsonb(q) ORDER BY q.request_revision) FROM book_ordinary_deletion_requests q JOIN target a ON a.book_id=q.book_id),
    'delete_decisions', (SELECT jsonb_agg(to_jsonb(q) ORDER BY q.request_revision) FROM book_ordinary_deletion_decisions q JOIN target a ON a.book_id=q.book_id),
    'active_storage', (SELECT jsonb_agg(to_jsonb(g) ORDER BY g.chapter_id,g.version_id) FROM free_edition_active_storage g JOIN target a ON a.chapter_id=g.chapter_id),
    'storage_generations', (SELECT jsonb_agg(to_jsonb(g) ORDER BY g.chapter_id,g.version_id,g.generation) FROM generations g),
    'pack_receipts', (SELECT jsonb_agg(to_jsonb(p) ORDER BY p.id) FROM free_storage_packs p WHERE p.id IN (SELECT pack_id FROM generations)),
    'layers', (SELECT jsonb_agg(to_jsonb(l) ORDER BY l.id) FROM commerce_layers l JOIN selected_layers s ON s.id=l.id),
    'layer_versions', (SELECT jsonb_agg(to_jsonb(v) ORDER BY v.id) FROM commerce_layer_versions v JOIN selected_layers l ON l.id=v.layer_id),
    'layer_memberships', (SELECT jsonb_agg(to_jsonb(vb) ORDER BY vb.layer_version_id,vb.book_id) FROM commerce_layer_version_books vb JOIN commerce_layer_versions v ON v.id=vb.layer_version_id JOIN selected_layers l ON l.id=v.layer_id),
    'layer_offers', (SELECT jsonb_agg(to_jsonb(o) ORDER BY o.id) FROM commerce_layer_offers o JOIN selected_layers l ON l.id=o.layer_id),
    'layer_terms', (SELECT jsonb_agg(to_jsonb(t) ORDER BY t.id) FROM terms t),
    'layer_revocations', (SELECT jsonb_agg(to_jsonb(r) ORDER BY r.term_id) FROM commerce_layer_term_revocations r JOIN terms t ON t.id=r.term_id),
    'layer_wallets', (SELECT jsonb_agg(to_jsonb(w) ORDER BY w.user_id) FROM reader_wallet_accounts w WHERE w.user_id IN (SELECT user_id FROM terms)),
    'layer_journal', (SELECT jsonb_agg(to_jsonb(e) ORDER BY e.id) FROM reader_wallet_entries e JOIN terms t ON t.id=e.layer_term_id),
    'layer_commands', (SELECT jsonb_agg(to_jsonb(c) ORDER BY c.user_id,c.command_key) FROM commerce_layer_commands c JOIN terms t ON t.id=c.term_id)
)::text;
