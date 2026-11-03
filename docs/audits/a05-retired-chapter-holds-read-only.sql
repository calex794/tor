-- A-05 / 2026-11-04: inspect legacy chapter holds without modifying data.
-- Run manually only with authorized read-only access, preferably off production.
-- This is NOT a new chapter moderation workflow and must not unblock chapters.
BEGIN TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY;

-- Scope and consistency: constraints should normally rule out mismatches.
SELECT
    COUNT(*) AS chapters_total,
    COUNT(*) FILTER (WHERE c.admin_hold) AS legacy_held_total,
    COUNT(*) FILTER (WHERE c.moderation_state = 'hold') AS generic_hold_total,
    COUNT(*) FILTER (WHERE c.moderation_state = 'hidden') AS hidden_total,
    COUNT(*) FILTER (WHERE c.moderation_state = 'blocked') AS blocked_total,
    COUNT(*) FILTER (
        WHERE (c.moderation_state = 'none') IS DISTINCT FROM (NOT c.admin_hold)
    ) AS inconsistent_hold_total
FROM chapters AS c;

-- Find affected books without exposing private text or personal identities.
SELECT
    c.book_id,
    b.visibility AS book_visibility,
    b.visibility_status AS book_moderation_status,
    b.admin_hold AS book_admin_hold,
    COUNT(*) AS held_chapters,
    COUNT(*) FILTER (WHERE c.visibility = 'public') AS publicly_marked,
    COUNT(*) FILTER (WHERE c.access_level = 'paid') AS paid_chapters,
    COUNT(*) FILTER (WHERE c.moderation_state IN ('hidden', 'blocked')) AS specific_decisions
FROM chapters AS c
JOIN books AS b ON b.id = c.book_id
WHERE c.admin_hold OR c.moderation_state <> 'none'
GROUP BY c.book_id, b.visibility, b.visibility_status, b.admin_hold
ORDER BY held_chapters DESC, c.book_id;

-- At most 100 affected chapter IDs and buyer-impact counts for triage.
-- Historical purchases and active library grants are distinct concepts.
SELECT
    c.id AS chapter_id,
    c.book_id,
    c.visibility,
    c.access_level,
    c.admin_hold,
    c.moderation_state,
    (SELECT COUNT(*) FROM chapter_publications AS p
     WHERE p.chapter_id = c.id) AS pinned_publications,
    (SELECT COUNT(*) FROM library_purchased_chapters AS l
     WHERE l.chapter_id = c.id) AS library_purchase_pointers,
    (SELECT COUNT(*) FROM chapter_purchase_facts AS f
     WHERE f.chapter_id = c.id) AS historical_purchase_facts,
    (SELECT COUNT(*) FROM moderation_chapter_hold_events AS e
     WHERE e.chapter_id = c.id) AS hold_audit_events,
    (SELECT COUNT(*) FROM moderation_chapter_decision_events AS e
     WHERE e.chapter_id = c.id) AS decision_audit_events
FROM chapters AS c
WHERE c.admin_hold OR c.moderation_state <> 'none'
ORDER BY c.book_id, c.id
LIMIT 100;

ROLLBACK;
