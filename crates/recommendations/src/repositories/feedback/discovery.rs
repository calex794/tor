//! Recommendation metadata visibility under verified session-derived actor identity.
//! Bind $1 as an Option<i64> on every query; anonymous traffic stays basic-only.
//! This CTE is read-side only. It does not grant reading or purchased rights.
pub(super) const ACTOR_DISCOVERY_CTE: &str = r#"
WITH rec_actor_books AS (
    SELECT b.id, b.title, b.created_at
    FROM books b
    LEFT JOIN book_recommendation_audiences a ON a.book_id=b.id
    WHERE b.visibility='public' AND b.visibility_status='active'
      AND b.lifecycle_status<>'pending_deletion'
      AND NOT b.admin_hold AND NOT b.is_deleted
      AND (
        (b.access_level='basic' AND COALESCE(a.audience,'general')='general')
        OR (b.access_level='adult' AND COALESCE(a.audience,'general') IN ('general','adult')
            AND EXISTS(SELECT 1 FROM users u WHERE u.id=$1 AND u.account_state='active' AND u.adult_enabled))
        OR (b.access_level='pro_adult' AND COALESCE(a.audience,'general') IN ('general','adult')
            AND EXISTS(SELECT 1 FROM users u JOIN user_level_grants g ON g.user_id=u.id
                       WHERE u.id=$1 AND u.account_state='active'
                         AND g.level='pro_adult' AND g.is_active))
      )
)
"#;
