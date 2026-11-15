//! D-05 storefront read models. No money mutations or delivery grants.
//! Public reads include only visible Book metadata; paid terms are actor-scoped.
use crate::domain::layers::LayerError;
use sqlx::{PgPool, Row};

#[derive(Clone, Debug)]
pub struct StorefrontBook {
    pub id: i64,
    pub title: String,
    pub has_cover: bool,
}

#[derive(Clone, Debug)]
pub struct LayerStorefront {
    pub id: i64,
    pub title: String,
    pub owner_id: i64,
    pub offer_id: i64,
    pub version_id: i64,
    pub price_minor: i64,
    pub term_kind: String,
    pub public_books: Vec<StorefrontBook>,
    pub public_book_count: i64,
    pub has_package_cover: bool,
    pub has_access: bool,
    pub has_lifetime_access: bool,
}

#[derive(Clone, Debug)]
pub struct LayerTermCard {
    pub layer_id: i64,
    pub title: String,
    pub term_id: i64,
    pub price_minor: i64,
    pub term_kind: String,
    pub starts_utc: String,
    pub expires_utc: Option<String>,
    pub state: String,
    pub first_readable_chapter: Option<i64>,
}

/// Bounded public listing; hidden, held and deleted Books cannot leak titles.
/// A published Layer with no public Books may be described but never sold by UI.
pub async fn storefront(
    pool: &PgPool,
    layer_id: i64,
    viewer: Option<i64>,
) -> Result<Option<LayerStorefront>, LayerError> {
    if layer_id <= 0 || viewer.is_some_and(|id| id <= 0) {
        return Err(LayerError::Invalid);
    }
    let head = sqlx::query(
        "SELECT l.id,l.title,l.owner_user_id,l.current_version_id,
                o.id AS offer_id,o.price_minor,o.term_kind,
                EXISTS(SELECT 1 FROM commerce_layer_cover_candidates c
                    WHERE c.layer_id=l.id AND c.status='approved') AS has_package_cover
         FROM commerce_layers l
         JOIN commerce_layer_offers o
           ON o.layer_id=l.id AND o.id=l.current_offer_id
         WHERE l.id=$1 AND l.status='active' AND l.current_version_id IS NOT NULL",
    )
    .bind(layer_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| LayerError::Unavailable)?;
    let Some(head) = head else {
        return Ok(None);
    };
    let version_id: i64 = head
        .try_get("current_version_id")
        .map_err(|_| LayerError::Unavailable)?;
    let rows = sqlx::query(
        "SELECT b.id,b.title,COUNT(*) OVER() AS total_visible,
                EXISTS (
                  SELECT 1 FROM book_cover_slots s
                  JOIN cover_variant_jobs j ON j.id=s.job_id AND j.book_id=b.id
                  JOIN cover_variants v ON v.job_id=j.id AND v.variant_name='card'
                  WHERE s.book_id=b.id AND j.status='ready'
                ) AS has_cover
         FROM commerce_layer_version_books vb
         JOIN books b ON b.id=vb.book_id
         WHERE vb.layer_version_id=$1 AND b.visibility='public' AND b.visibility_status='active'
           AND b.lifecycle_status NOT IN ('pending_deletion','ordinary_deleted')
           AND NOT b.admin_hold AND NOT b.is_deleted
           AND (
             b.access_level='basic'
             OR (b.access_level='adult' AND EXISTS (
               SELECT 1 FROM users viewer WHERE viewer.id=$2
                 AND viewer.account_state='active' AND viewer.adult_enabled
             ))
             OR (b.access_level='pro_adult' AND EXISTS (
               SELECT 1 FROM users viewer JOIN user_level_grants age
                 ON age.user_id=viewer.id AND age.level='pro_adult' AND age.is_active
               WHERE viewer.id=$2 AND viewer.account_state='active'
             ))
           )
         ORDER BY b.id LIMIT 40",
    )
    .bind(version_id)
    .bind(viewer)
    .fetch_all(pool)
    .await
    .map_err(|_| LayerError::Unavailable)?;
    let public_book_count = rows
        .first()
        .map(|row| row.try_get("total_visible"))
        .transpose()
        .map_err(|_| LayerError::Unavailable)?
        .unwrap_or(0_i64);
    let mut books = Vec::with_capacity(rows.len());
    for row in rows {
        books.push(StorefrontBook {
            id: row.try_get("id").map_err(|_| LayerError::Unavailable)?,
            title: row.try_get("title").map_err(|_| LayerError::Unavailable)?,
            has_cover: row
                .try_get("has_cover")
                .map_err(|_| LayerError::Unavailable)?,
        });
    }
    // An active 30-day term is not an owned lifetime term. The checkout
    // allows the reader to purchase lifetime even while prepaid access works.
    let (has_access, has_lifetime_access): (bool, bool) = if let Some(actor) = viewer {
        sqlx::query_as(
            "SELECT COALESCE(BOOL_OR(
                       t.started_at<=now()
                       AND (t.expires_at IS NULL OR t.expires_at>now())
                    ),FALSE),
                    COALESCE(BOOL_OR(t.term_kind='lifetime'),FALSE)
             FROM commerce_layer_terms t
             WHERE t.user_id=$1 AND t.layer_id=$2
               AND NOT EXISTS(
                   SELECT 1 FROM commerce_layer_term_revocations r WHERE r.term_id=t.id
               )",
        )
        .bind(actor)
        .bind(layer_id)
        .fetch_one(pool)
        .await
        .map_err(|_| LayerError::Unavailable)?
    } else {
        (false, false)
    };
    Ok(Some(LayerStorefront {
        id: layer_id,
        title: head.try_get("title").map_err(|_| LayerError::Unavailable)?,
        owner_id: head
            .try_get("owner_user_id")
            .map_err(|_| LayerError::Unavailable)?,
        offer_id: head
            .try_get("offer_id")
            .map_err(|_| LayerError::Unavailable)?,
        version_id,
        price_minor: head
            .try_get("price_minor")
            .map_err(|_| LayerError::Unavailable)?,
        term_kind: head
            .try_get("term_kind")
            .map_err(|_| LayerError::Unavailable)?,
        public_books: books,
        public_book_count,
        has_package_cover: head
            .try_get("has_package_cover")
            .map_err(|_| LayerError::Unavailable)?,
        has_access,
        has_lifetime_access,
    }))
}

/// A read-only alternative to a blocked direct paid-chapter sale.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExclusivePackageLink {
    pub layer_id: i64,
    pub title: String,
    /// UTC deadline for timed early access; None means package-only indefinitely.
    pub until_utc: Option<String>,
}

/// Show a real package link only when the book, the optional paid chapter,
/// and the current package offer are public and accessible to this viewer.
/// An unauthorized age tier, a hidden package, unpublished chapters or
/// unattached paid editions must never be described as purchasable.
pub async fn exclusive_package_link(
    pool: &PgPool,
    book_id: i64,
    chapter_id: Option<i64>,
    viewer: Option<i64>,
) -> Result<Option<ExclusivePackageLink>, LayerError> {
    if book_id <= 0 || chapter_id.is_some_and(|id| id <= 0) || viewer.is_some_and(|id| id <= 0) {
        return Ok(None);
    }
    let row = sqlx::query(
        "SELECT l.id AS layer_id,l.title AS layer_title,
                CASE WHEN b.package_only_until IS NOT NULL
                     THEN TO_CHAR(b.package_only_until AT TIME ZONE 'UTC','DD.MM.YYYY HH24:MI')
                     ELSE NULL END AS until_utc
         FROM books b
         JOIN commerce_layers l ON l.id=b.package_only_layer_id
           AND l.status='active' AND l.current_version_id IS NOT NULL
         JOIN commerce_layer_offers offer ON offer.id=l.current_offer_id AND offer.layer_id=l.id
         JOIN commerce_layer_version_books vb
           ON vb.layer_version_id=l.current_version_id AND vb.book_id=b.id
         WHERE b.id=$1 AND (b.package_only_until IS NULL OR b.package_only_until>now())
           AND b.visibility='public' AND b.visibility_status='active'
           AND b.lifecycle_status NOT IN ('pending_deletion','ordinary_deleted')
           AND NOT b.admin_hold AND NOT b.is_deleted
           AND (
             b.access_level='basic'
             OR (b.access_level='adult' AND EXISTS(
               SELECT 1 FROM users u WHERE u.id=$3 AND u.account_state='active' AND u.adult_enabled
             ))
             OR (b.access_level='pro_adult' AND EXISTS(
               SELECT 1 FROM users u JOIN user_level_grants g
                 ON g.user_id=u.id AND g.level='pro_adult' AND g.is_active
               WHERE u.id=$3 AND u.account_state='active'
             ))
           )
           AND EXISTS (
             SELECT 1 FROM chapters c
             JOIN chapter_publications p ON p.chapter_id=c.id
             JOIN chapter_versions v ON v.id=p.version_id AND v.chapter_id=c.id
             JOIN storage_objects obj ON obj.id=v.storage_object_id
               AND obj.state='attached' AND obj.object_kind='chapter'
             WHERE c.book_id=b.id AND ($2::BIGINT IS NULL OR c.id=$2)
               AND c.visibility='public' AND c.access_level='paid' AND c.price_amount>0
               AND NOT c.admin_hold AND NOT c.is_deleted
               AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
           )
         LIMIT 1",
    )
    .bind(book_id)
    .bind(chapter_id)
    .bind(viewer)
    .fetch_optional(pool)
    .await
    .map_err(|_| LayerError::Unavailable)?;
    row.map(|r| {
        Ok(ExclusivePackageLink {
            layer_id: r.try_get("layer_id").map_err(|_| LayerError::Unavailable)?,
            title: r
                .try_get("layer_title")
                .map_err(|_| LayerError::Unavailable)?,
            until_utc: r
                .try_get("until_utc")
                .map_err(|_| LayerError::Unavailable)?,
        })
    })
    .transpose()
}

/// Resolve the owning public Book inside commerce. Web must not issue SQL.
pub async fn exclusive_package_for_chapter(
    pool: &PgPool,
    chapter_id: i64,
    viewer: Option<i64>,
) -> Result<Option<(i64, ExclusivePackageLink)>, LayerError> {
    if chapter_id <= 0 {
        return Ok(None);
    }
    let book_id: Option<i64> = sqlx::query_scalar(
        "SELECT book_id FROM chapters WHERE id=$1",
    )
    .bind(chapter_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| LayerError::Unavailable)?;
    let Some(book_id) = book_id else {
        return Ok(None);
    };
    Ok(exclusive_package_link(pool, book_id, Some(chapter_id), viewer)
        .await?
        .map(|link| (book_id, link)))
}

/// Historical terms are private even if a Layer was later hidden.
/// Future prepaid renewals are shown as pending; refund facts mark revoked.
pub async fn purchased_terms(actor: i64, pool: &PgPool) -> Result<Vec<LayerTermCard>, LayerError> {
    if actor <= 0 {
        return Err(LayerError::Invalid);
    }
    let rows = sqlx::query(
        "SELECT t.layer_id,l.title,t.id AS term_id,t.amount_minor,t.term_kind,
                TO_CHAR(t.started_at AT TIME ZONE 'UTC','DD.MM.YYYY HH24:MI') AS starts_utc,
                TO_CHAR(t.expires_at AT TIME ZONE 'UTC','DD.MM.YYYY HH24:MI') AS expires_utc,
                preview.chapter_id AS first_readable_chapter,
                CASE WHEN r.term_id IS NOT NULL THEN 'revoked'
                     WHEN t.started_at>now() THEN 'pending'
                     WHEN t.expires_at IS NOT NULL AND t.expires_at<=now() THEN 'expired'
                     ELSE 'active' END AS term_state
         FROM commerce_layer_terms t
         JOIN commerce_layers l ON l.id=t.layer_id
         LEFT JOIN commerce_layer_term_revocations r ON r.term_id=t.id
         LEFT JOIN LATERAL (
           SELECT c.id AS chapter_id
           FROM chapters c
           JOIN books b ON b.id=c.book_id
           JOIN chapter_publications cp ON cp.chapter_id=c.id
           WHERE r.term_id IS NULL
             AND t.started_at<=now()
             AND (t.expires_at IS NULL OR t.expires_at>now())
             AND b.visibility='public' AND b.visibility_status='active' AND NOT b.admin_hold AND NOT b.is_deleted
             AND (
               b.access_level='basic'
               OR (b.access_level='adult' AND EXISTS (
                 SELECT 1 FROM users viewer WHERE viewer.id=t.user_id
                   AND viewer.account_state='active' AND viewer.adult_enabled
               ))
               OR (b.access_level='pro_adult' AND EXISTS (
                 SELECT 1 FROM users viewer JOIN user_level_grants age
                   ON age.user_id=viewer.id AND age.level='pro_adult' AND age.is_active
                 WHERE viewer.id=t.user_id AND viewer.account_state='active'
               ))
             )
             AND c.visibility='public' AND c.access_level='paid'
             AND c.price_amount>0 AND NOT c.admin_hold AND NOT c.is_deleted
             AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
             AND EXISTS (
               SELECT 1 FROM commerce_layer_versions origin
               JOIN commerce_layer_versions v ON v.layer_id=origin.layer_id
                    AND v.version_no>=origin.version_no
                    AND (t.expires_at IS NULL OR v.created_at<t.expires_at)
               JOIN commerce_layer_version_books vb ON vb.layer_version_id=v.id
                    AND vb.book_id=b.id
               WHERE origin.id=t.access_origin_version_id AND origin.layer_id=t.layer_id
             )
           ORDER BY c.id LIMIT 1
         ) preview ON TRUE
         WHERE t.user_id=$1
         ORDER BY t.purchased_at DESC,t.id DESC LIMIT 50",
    )
    .bind(actor)
    .fetch_all(pool)
    .await
    .map_err(|_| LayerError::Unavailable)?;
    rows.into_iter()
        .map(|row| {
            Ok(LayerTermCard {
                layer_id: row
                    .try_get("layer_id")
                    .map_err(|_| LayerError::Unavailable)?,
                title: row.try_get("title").map_err(|_| LayerError::Unavailable)?,
                term_id: row
                    .try_get("term_id")
                    .map_err(|_| LayerError::Unavailable)?,
                price_minor: row
                    .try_get("amount_minor")
                    .map_err(|_| LayerError::Unavailable)?,
                term_kind: row
                    .try_get("term_kind")
                    .map_err(|_| LayerError::Unavailable)?,
                starts_utc: row
                    .try_get("starts_utc")
                    .map_err(|_| LayerError::Unavailable)?,
                expires_utc: row
                    .try_get("expires_utc")
                    .map_err(|_| LayerError::Unavailable)?,
                state: row
                    .try_get("term_state")
                    .map_err(|_| LayerError::Unavailable)?,
                first_readable_chapter: row
                    .try_get("first_readable_chapter")
                    .map_err(|_| LayerError::Unavailable)?,
            })
        })
        .collect()
}
